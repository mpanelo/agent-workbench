use std::{io, sync::Arc, time::Duration};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use tokio::{
    sync::watch,
    task::JoinSet,
    time::{self, MissedTickBehavior},
};
use workbench_core::{ActionError, Engine, Snapshot, WorkItemState, validate_agent_input};

mod cleanup;
mod cli;
mod help;
mod interaction;
mod listing;
mod maintenance;
mod preview;
mod registration;
mod removal;
mod review;
mod syntax;
mod theme;
mod ui;
mod wrap;

type DiscoveryState = Option<Result<Snapshot, String>>;

#[derive(Clone, Debug, Default)]
struct AppState {
    discovery: DiscoveryState,
    work_items: Option<Result<Vec<WorkItemState>, String>>,
}

impl AppState {
    fn apply_detail_edit(
        &mut self,
        mutation: &maintenance::Mutation,
        interaction: &mut interaction::Interaction,
    ) {
        if let maintenance::Mutation::Details {
            expected,
            id,
            description,
        } = mutation
        {
            if let Some(Ok(items)) = &mut self.work_items {
                for state in items {
                    if state.item == *expected {
                        state.item.id.clone_from(id);
                        state.item.title.clone_from(description);
                    }
                }
            }
            if interaction.selected_id.as_ref() == Some(&expected.id) {
                interaction.selected_id = Some(id.clone());
            }
        }
    }

    /// Reconcile fresh registration data against observations, preventing an
    /// in-flight discovery snapshot from resurrecting a removed/edited entry.
    fn reload_registry(&mut self, engine: &Engine) {
        let previous = self.items().to_vec();
        self.work_items = Some(
            engine
                .work_item_states(self.snapshot())
                .map(|mut items| {
                    for current in &mut items {
                        if let Some(old) = previous.iter().find(|old| {
                            let mut observed = old.item.clone();
                            observed.title.clone_from(&current.item.title);
                            observed == current.item && old.pane == current.pane
                        }) {
                            current.status = old.status;
                            current.status_detail.clone_from(&old.status_detail);
                            current.input_prompt.clone_from(&old.input_prompt);
                        }
                    }
                    items
                })
                .map_err(|error| error.to_string()),
        );
    }
    fn snapshot(&self) -> Option<&Snapshot> {
        self.discovery
            .as_ref()
            .and_then(|state| state.as_ref().ok())
    }
    fn from_refresh(
        discovery: Result<Snapshot, String>,
        work_items: Result<Vec<WorkItemState>, String>,
    ) -> Self {
        Self {
            discovery: Some(discovery),
            work_items: Some(work_items),
        }
    }

    fn items(&self) -> &[WorkItemState] {
        self.work_items
            .as_ref()
            .and_then(|items| items.as_ref().ok())
            .map_or(&[], Vec::as_slice)
    }
}

enum RunExit {
    Quit,
    Focus(String),
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::process::ExitCode {
    match start().await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Error: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn start() -> io::Result<()> {
    let options = cli::parse(std::env::args_os().skip(1)).map_err(io::Error::other)?;
    if options.command == cli::Command::Help {
        print!("{}", cli::HELP);
        return Ok(());
    }
    if matches!(
        options.command,
        cli::Command::CodexHook | cli::Command::CodexNotify(_)
    ) {
        // Observational hooks must always return neutral JSON, even on failure.
        // Never emit policy decisions, model context, or continuation requests.
        let result = codex_callback(&options).await;
        if let Err(error) = result {
            eprintln!("Workbench signal ignored: {error}");
        }
        println!("{{}}");
        return Ok(());
    }
    let engine = Arc::new(Engine::new(options.state_path().map_err(io::Error::other)?));
    match options.command {
        cli::Command::Register(item) => {
            let id = item.id.clone();
            engine.register_work_item(item).map_err(io::Error::other)?;
            println!("Registered {id}.");
            return Ok(());
        }
        cli::Command::List => {
            // Listing remains useful offline, with unavailable/UNKNOWN observations.
            let snapshot = engine.discover().await.ok();
            let items = engine
                .observe_work_item_states(snapshot.as_ref())
                .await
                .map_err(io::Error::other)?;
            if items.is_empty() {
                println!("No work items registered.");
            }
            for state in items {
                let item = state.item;
                println!(
                    "{}\t{}\t{}\t{} ({})\n  {}\n  Status: {}\n  Repository: {}\n  Workspace: {}\n  Branch: {}",
                    item.id,
                    state.status,
                    item.kind,
                    item.pane_id,
                    state.pane,
                    item.title,
                    state.status_detail,
                    ui::display_path(&item.repository),
                    ui::display_path(&item.workspace),
                    item.branch.as_deref().unwrap_or("unavailable")
                );
            }
            return Ok(());
        }
        cli::Command::Run => {}
        cli::Command::Help => unreachable!("help was handled above"),
        cli::Command::CodexHook | cli::Command::CodexNotify(_) => {
            unreachable!("callback handled above")
        }
    }
    let mut interaction = interaction::Interaction::default();
    let mut reviews = review::ReviewUi::new(options.diff_base);
    loop {
        let mut terminal = init_terminal()?;
        let result = run(
            &mut terminal,
            Arc::clone(&engine),
            &mut interaction,
            &mut reviews,
        )
        .await;
        restore_terminal()?;
        match result? {
            RunExit::Quit => return Ok(()),
            RunExit::Focus(id) => {
                interaction.finish_focus(&id, engine.focus_work_item(&id).await);
            }
        }
    }
}

async fn codex_callback(options: &cli::Options) -> io::Result<()> {
    use std::io::Read;
    const LIMIT: usize = 128 * 1024;
    let engine = Engine::new(options.state_path().map_err(io::Error::other)?);
    let (payload, notification) = match &options.command {
        cli::Command::CodexNotify(json) => (json.clone(), true),
        _ => {
            let mut bytes = String::new();
            io::stdin()
                .lock()
                .take((LIMIT + 1) as u64)
                .read_to_string(&mut bytes)?;
            (bytes, false)
        }
    };
    // Interrupt hooks have a short budget; never stall the agent on tmux/ps.
    time::timeout(
        Duration::from_millis(800),
        engine.record_codex_signal(&payload, notification),
    )
    .await
    .map_err(io::Error::other)??;
    Ok(())
}

fn init_terminal() -> io::Result<ratatui::DefaultTerminal> {
    let terminal = match ratatui::try_init() {
        Ok(terminal) => terminal,
        Err(error) => {
            ratatui::restore();
            return Err(error);
        }
    };
    if let Err(error) = crossterm::execute!(io::stdout(), event::EnableBracketedPaste) {
        ratatui::restore();
        return Err(error);
    }
    Ok(terminal)
}

fn restore_terminal() -> io::Result<()> {
    let paste = crossterm::execute!(io::stdout(), event::DisableBracketedPaste);
    let restore = ratatui::try_restore();
    paste.and(restore)
}

async fn run(
    terminal: &mut ratatui::DefaultTerminal,
    engine: Arc<Engine>,
    interaction: &mut interaction::Interaction,
    reviews: &mut review::ReviewUi,
) -> io::Result<RunExit> {
    let (sender, mut receiver) = watch::channel::<AppState>(AppState::default());
    let discovery_engine = Arc::clone(&engine);
    let discovery = tokio::spawn(async move {
        let mut refresh = time::interval(Duration::from_secs(2));
        refresh.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            refresh.tick().await;
            let discovery = discovery_engine
                .discover()
                .await
                .map_err(|error| error.to_string());
            let work_items = discovery_engine
                .observe_work_item_states(discovery.as_ref().ok())
                .await
                .map_err(|error| error.to_string());
            if sender
                .send(AppState::from_refresh(discovery, work_items))
                .is_err()
            {
                break;
            }
        }
    });
    let result = event_loop(terminal, &mut receiver, engine, interaction, reviews).await;
    discovery.abort();
    // Wait for cancellation so an in-flight command is dropped and killed.
    let _ = discovery.await;
    result
}

async fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    receiver: &mut watch::Receiver<AppState>,
    engine: Arc<Engine>,
    interaction: &mut interaction::Interaction,
    reviews: &mut review::ReviewUi,
) -> io::Result<RunExit> {
    let mut state = AppState::default();
    let mut scroll = 0_u16;
    let mut redraw = true;
    let mut sends = JoinSet::new();
    let mut approvals = JoinSet::new();
    let mut diffs = JoinSet::new();
    let mut review_saves = JoinSet::new();
    let mut preparations = JoinSet::new();
    let mut registrations = JoinSet::<Result<workbench_core::WorkItem, String>>::new();
    let mut registration = registration::RegistrationUi::default();
    let mut maintenance = maintenance::MaintenanceUi::default();
    let mut help = help::HelpUi::default();
    let mut cleanup = cleanup::CleanupUi::default();
    let mut removal = removal::RemovalUi::default();
    let mut cleanup_preparations = JoinSet::new();
    let mut cleanups = JoinSet::<(u64, String, Result<(), String>)>::new();
    let mut mutations = JoinSet::<(Box<maintenance::Mutation>, Result<(), String>)>::new();
    let mut previews = JoinSet::new();
    let (preview_updates, mut preview_receiver) = watch::channel(None);
    let mut highlights = JoinSet::new();
    let mut input_tick = time::interval(Duration::from_millis(100));
    input_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        if let Some(request) = reviews.request_syntax() {
            highlights.abort_all();
            let engine = Arc::clone(&engine);
            highlights.spawn(async move {
                let ticket = request.ticket;
                (ticket, request.run(&engine).await)
            });
        }
        removal.observe(
            state
                .work_items
                .as_ref()
                .and_then(|result| result.as_deref().ok()),
        );
        let preview_item = if !help.is_open()
            && !cleanup.is_open()
            && !maintenance.is_open()
            && registration.pane.is_none()
            && !reviews.is_open()
            && !removal.is_open()
        {
            interaction
                .selected(state.items())
                .filter(|state| state.pane == workbench_core::PaneAvailability::Present)
                .map(|state| &state.item)
        } else {
            None
        };
        if interaction.preview.select(preview_item) {
            previews.abort_all();
            preview_updates.send_replace(None);
            if let Some((ticket, item)) = interaction.preview.target() {
                let engine = Arc::clone(&engine);
                let updates = preview_updates.clone();
                previews.spawn(async move {
                    let result = engine
                        .stream_preview(&item, |capture| {
                            updates.send_replace(Some((
                                ticket,
                                capture.map_err(|error| error.to_string()),
                            )));
                        })
                        .await;
                    (ticket, result.map_err(|error| error.to_string()))
                });
            }
            redraw = true;
        }
        if redraw {
            terminal.draw(|frame| {
                if help.is_open() {
                    help.render(frame);
                } else if cleanup.is_open() {
                    cleanup.render(frame);
                } else if maintenance.is_open() {
                    maintenance.render(frame);
                } else if registration.pane.is_some() {
                    registration.render(frame);
                } else if reviews.is_open() {
                    reviews.render(frame);
                } else {
                    ui::render_work(frame, &state, &mut scroll, interaction);
                    removal.render(frame);
                }
            })?;
            redraw = false;
        }
        tokio::select! {
            changed = preview_receiver.changed() => {
                if changed.is_ok()
                    && let Some((ticket, capture)) = preview_receiver.borrow_and_update().clone() {
                    redraw |= interaction.preview.finish(ticket, capture);
                }
            },
            completed = previews.join_next(), if !previews.is_empty() => {
                redraw |= match completed {
                    Some(Ok((ticket, result))) if interaction.preview.owns(ticket) => {
                        preview_updates.send_replace(None);
                        interaction.preview.finish(ticket, Err(result.err().unwrap_or_else(|| "Preview stream stopped.".into())))
                    },
                    Some(Err(error)) if !error.is_cancelled() && previews.is_empty() => interaction.preview.fail(),
                    _ => false,
                };
            }
            completed = approvals.join_next(), if !approvals.is_empty() => {
                match completed {
                    Some(Ok((request, result))) => interaction.finish_approval(&request, result),
                    _ => interaction.fail_approval_task(),
                }
                redraw = true;
            }
            completed = cleanup_preparations.join_next(), if !cleanup_preparations.is_empty() => {
                match completed {
                    Some(Ok((ticket, result))) => cleanup.finish_preview(ticket, result),
                    Some(Err(error)) if !error.is_cancelled() => cleanup.fail("Cleanup preview stopped; cancel and reopen.".into()),
                    _ => {},
                }
                redraw = true;
            }
            completed = cleanups.join_next(), if !cleanups.is_empty() => {
                match completed {
                    Some(Ok((ticket, id, result))) => {
                        let success = result.is_ok();
                        cleanup.finish_execution(ticket, result);
                        if success {
                            state.reload_registry(&engine);
                            state.sync_selection(interaction);
                            interaction.reveal_selection = true;
                            interaction.message = Some(interaction::Message::confirmation(
                                format!("Cleaned up {id}: workmux window and worktree removed; branch and review history kept."),
                                std::time::Instant::now(),
                            ));
                        }
                    }
                    _ => cleanup.fail("Cleanup task stopped. Resources may be partly removed; inspect them before retrying. Registration was kept.".into()),
                }
                redraw = true;
            }
            completed = mutations.join_next(), if !mutations.is_empty() => {
                match completed {
                    Some(Ok((mutation, result))) => {
                        let success = result.is_ok();
                        if matches!(mutation.as_ref(), maintenance::Mutation::Unregister(_)) {
                            removal.finish_unregister(result);
                        } else {
                            maintenance.finish(result);
                        }
                        if success {
                            state.apply_detail_edit(&mutation, interaction);
                            state.reload_registry(&engine);
                            state.sync_selection(interaction);
                            interaction.reveal_selection = true;
                            interaction.message = None;
                        }
                    }
                    _ if removal.is_saving() => removal.finish_unregister(Err("Unregister task stopped unexpectedly; inspect WORK before retrying.".into())),
                    _ => maintenance.finish(Err("Maintenance task stopped unexpectedly; inspect WORK before retrying.".into())),
                }
                redraw = true;
            }
            completed = preparations.join_next(), if !preparations.is_empty() => {
                match completed {
                    Some(Ok((ticket, result))) => registration.finish(ticket, result),
                    Some(Err(error)) if !error.is_cancelled() => registration.finish(registration.generation, Err("Pane discovery task stopped; cancel and retry.".into())),
                    _ => {},
                }
                redraw = true;
            }
            completed = registrations.join_next(), if !registrations.is_empty() => {
                registration.saving = false;
                match completed {
                    Some(Ok(Ok(item))) => {
                        registration.close();
                        // Make the saved item immediately visible. Retain observations
                        // for unchanged existing items until the next normal refresh.
                        state.reload_registry(&engine);
                        interaction.selected_pane = None;
                        scroll = 0;
                        interaction.selected_id = Some(item.id.clone());
                        interaction.reveal_selection = true;
                        interaction.message = None;
                    }
                    Some(Ok(Err(error))) => registration.error = Some(error),
                    _ => registration.error = Some("Registration task stopped unexpectedly; inspect WORK before retrying.".into()),
                }
                redraw = true;
            }
            completed = diffs.join_next(), if !diffs.is_empty() => {
                match completed {
                    Some(Ok((ticket, result))) => reviews.finish(ticket, result),
                    Some(Err(error)) if !error.is_cancelled() => reviews.fail_loading("Git capture task stopped unexpectedly; reload the diff.".into()),
                    _ => {},
                }
                redraw = true;
            }
            completed = highlights.join_next(), if !highlights.is_empty() => {
                if let Some(Ok((ticket, lines))) = completed {
                    reviews.finish_syntax(ticket, lines);
                }
                redraw = true;
            }
            completed = review_saves.join_next(), if !review_saves.is_empty() => {
                match completed {
                    Some(Ok((ticket, result))) => reviews.finish_save(ticket, result),
                    _ => reviews.fail_saving("Review save task stopped; reopen review to verify the saved state before retrying.".into()),
                }
                redraw = true;
            }
            // Apply a local mutation before consuming the next watcher snapshot.
            changed = receiver.changed(), if mutations.is_empty() => {
                changed.map_err(|_| io::Error::other("Discovery task stopped unexpectedly"))?;
                let updated = receiver.borrow_and_update().clone();
                state = updated;
                state.reload_registry(&engine);
                state.sync_selection(interaction);
                redraw = true;
            }
            completed = sends.join_next(), if !sends.is_empty() => {
                interaction.sending = false;
                match completed {
                    Some(Ok((id, result))) => match result {
                        Ok(()) => {
                            interaction.message = Some(interaction::Message::confirmation(
                                format!("Reply submitted to {id}."), std::time::Instant::now(),
                            ));
                            interaction.draft = None;
                        }
                        Err(error) => interaction.message = Some(format!("Reply to {id} failed: {error}. Not retried; check the pane before resending.").into()),
                    },
                    _ => interaction.message = Some("Input task stopped unexpectedly. Check the pane before resending.".into()),
                }
                redraw = true;
            }
            _ = input_tick.tick() => {
                redraw |= interaction.tick_message(std::time::Instant::now());
                redraw |= interaction.tick_approval_feedback(std::time::Instant::now());
                while event::poll(Duration::ZERO)? {
                    redraw = true;
                    let event = event::read()?;
                    if matches!(event, Event::Resize(_, _)) { interaction.reveal_selection = true; }
                    let context = if removal.is_open() { removal.help_context() } else if cleanup.is_open() { help::Context::Cleanup } else { maintenance.help_context().unwrap_or_else(|| {
                        if registration.pane.is_some() { help::Context::Registration }
                        else if reviews.is_open() { help::Context::Review }
                        else if interaction.draft.is_some() { help::Context::Reply }
                        else if let Some(bar) = &interaction.response_bar { help::Context::Response { changed: bar.changed, can_reject: bar.can_reject() } }
                        else { interaction.help_context() }
                    }) };
                    if help.event(&event, context) {
                        continue;
                    }
                    if removal.is_open() {
                        removal.observe(state.work_items.as_ref().and_then(|result| result.as_deref().ok()));
                        if let Event::Key(key) = event {
                            match open_removal_action(removal.key(key), &mut cleanup) {
                                Some(RemovalAction::Unregister(mutation)) => {
                                    let engine = Arc::clone(&engine);
                                    mutations.spawn_blocking(move || { let result = mutation.execute(&engine); (mutation, result) });
                                }
                                Some(RemovalAction::CleanupPreview(ticket, item)) => {
                                    let engine = Arc::clone(&engine);
                                    cleanup_preparations.spawn(async move { (ticket, engine.prepare_cleanup(&item).await.map_err(|e| e.to_string())) });
                                }
                                None => {},
                            }
                        }
                        continue;
                    }
                    if cleanup.is_open() {
                        if let Event::Key(key) = event && key.kind != KeyEventKind::Release {
                            match cleanup.key(key) {
                                cleanup::Intent::Cancel => cleanup_preparations.abort_all(),
                                cleanup::Intent::Confirm(preview) => {
                                    let ticket = cleanup.generation;
                                    let id = preview.details.work_id.clone();
                                    let engine = Arc::clone(&engine);
                                    cleanups.spawn(async move { (ticket, id, engine.cleanup_work_item(&preview).await.map_err(|e| e.to_string())) });
                                }
                                cleanup::Intent::None => {},
                            }
                        }
                        continue;
                    }
                    if maintenance.is_open() {
                        match event {
                            Event::Paste(text) => maintenance.paste(&text),
                            Event::Key(key) if key.kind != KeyEventKind::Release => {
                                if let maintenance::Intent::Save(mutation) = maintenance.key(key) {
                                    let engine = Arc::clone(&engine);
                                    mutations.spawn_blocking(move || { let result = mutation.execute(&engine); (mutation, result) });
                                }
                            }
                            _ => {},
                        }
                        continue;
                    }
                    if registration.pane.is_some() {
                        match event {
                            Event::Paste(text) => registration.paste(&text),
                            Event::Key(key) if key.kind != KeyEventKind::Release => {
                                match registration.key(key) {
                                    registration::RegistrationIntent::Cancel => preparations.abort_all(),
                                    registration::RegistrationIntent::Save(draft) => {
                                        let engine = Arc::clone(&engine);
                                        registrations.spawn(async move { engine.register_discovered_work_item(*draft).await.map_err(|error| error.to_string()) });
                                    }
                                    registration::RegistrationIntent::None => {},
                                }
                            }
                            _ => {},
                        }
                        continue;
                    }
                    if let Event::Paste(text) = &event
                        && !interaction.sending
                        && let Some(draft) = &mut interaction.draft
                        && let Err(error) = draft.insert(text)
                    {
                        interaction.message = Some(error.into());
                    }
                    if let Event::Key(key) = event {
                        if key.kind == KeyEventKind::Release {
                            continue;
                        }
                        if reviews.is_open() {
                            if !reviews.is_saving() && (key.code == KeyCode::Char('q') || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))) {
                                return Ok(RunExit::Quit);
                            }
                            match reviews.key(key, terminal.size()?.height) {
                              review::ReviewIntent::Reload => {
                                diffs.abort_all();
                                let (ticket, id) = reviews.reload().expect("review is open");
                                let engine = Arc::clone(&engine);
                                let base = reviews.base.clone();
                                diffs.spawn(async move { (ticket, engine.open_review(&id, base.as_deref()).await.map_err(|error| error.to_string())) });
                              }
                              review::ReviewIntent::Save { ticket, mut session, path, reviewed } => {
                                let engine = Arc::clone(&engine);
                                review_saves.spawn_blocking(move || {
                                    let result = engine.set_file_reviewed(&mut session, &path, reviewed).map(|()| *session).map_err(|error| error.to_string());
                                    (ticket, result)
                                });
                              }
                              review::ReviewIntent::None => {},
                            }
                            if !reviews.is_open() { diffs.abort_all(); }
                            continue;
                        }
                        if let Some(draft) = &mut interaction.draft {
                            match key.code {
                                KeyCode::Esc | KeyCode::Char('c') if key.code == KeyCode::Esc || key.modifiers.contains(KeyModifiers::CONTROL) => {
                                    if !interaction.sending { interaction.draft = None; interaction.message = None; }
                                }
                                KeyCode::Enter if !interaction.sending => {
                                    match validate_agent_input(&draft.text) {
                                        Err(error) => interaction.message = Some(error.to_string().into()),
                                        Ok(()) => {
                                            let id = draft.item_id.clone();
                                            let text = draft.text.clone();
                                            let engine = Arc::clone(&engine);
                                            interaction.sending = true;
                                            interaction.message = Some(format!("Sending reply to {id}…").into());
                                            sends.spawn(async move { let result = engine.send_agent_input(&id, &text).await; (id, result) });
                                        }
                                    }
                                }
                                _ if !interaction.sending => if let Err(error) = draft.edit(key) { interaction.message = Some(error.into()); },
                                _ => {}
                            }
                            continue;
                        }
                        if interaction.sending { continue; }
                        match key.code {
                            KeyCode::Char('q') | KeyCode::Esc if interaction.response_bar.is_none() => return Ok(RunExit::Quit),
                            KeyCode::Char('c') if interaction.response_bar.is_none() && key.modifiers.contains(KeyModifiers::CONTROL) => return Ok(RunExit::Quit),
                            _ => {
                                if let Some(id) = navigate(key, &state, &mut scroll, interaction)
                                    && let Some(exit) = finish_switch(&id, interaction, engine.try_switch_work_item(&id).await) {
                                        return Ok(exit);
                                    }

                                if let Some(request) = interaction.approval_requested.take() {
                                    interaction.begin_approval(&request);
                                    let engine = Arc::clone(&engine);
                                    approvals.spawn(async move {
                                        let result = engine.respond_to_approval(&request).await;
                                        (request, result)
                                    });
                                }
                                if let Some(request) = interaction.maintenance_requested.take() {
                                    interaction.message = None;
                                    maintenance.open(request);
                                }
                                if let Some(item) = interaction.removal_requested.take() {
                                    interaction.message = None;
                                    removal.open(item);
                                }
                                if let Some(id) = interaction.review_requested.take()
                                    && let Some(ticket) = reviews.open(id.clone()) {
                                        diffs.abort_all();
                                        let engine = Arc::clone(&engine);
                                        let base = reviews.base.clone();
                                        diffs.spawn(async move { (ticket, engine.open_review(&id, base.as_deref()).await.map_err(|error| error.to_string())) });
                                }
                                if let Some(pane) = interaction.registration_requested.take() {
                                    let ticket = registration.open(pane.clone());
                                    let engine = Arc::clone(&engine);
                                    preparations.abort_all();
                                    preparations.spawn(async move { (ticket, engine.prepare_pane_registration(&pane).await.map_err(|error| error.to_string())) });
                                }
                            },
                        }
                    }
                }
            }
        }
    }
}

/// Only an external attachment needs to leave the event loop and restore the
/// terminal. A switch (including a failed one) keeps the display and watcher alive.
fn finish_switch(
    id: &str,
    interaction: &mut interaction::Interaction,
    result: Result<bool, ActionError>,
) -> Option<RunExit> {
    match result {
        Ok(false) => Some(RunExit::Focus(id.into())),
        result => {
            interaction.finish_focus(id, result.map(|_| ()));
            None
        }
    }
}

enum RemovalAction {
    Unregister(Box<maintenance::Mutation>),
    CleanupPreview(u64, workbench_core::WorkItem),
}

/// Unregister directly against the captured registration. Workspace deletion
/// still needs its read-only core preview and separate safety confirmation.
fn open_removal_action(
    intent: removal::Intent,
    cleanup: &mut cleanup::CleanupUi,
) -> Option<RemovalAction> {
    match intent {
        removal::Intent::Unregister(item) => Some(RemovalAction::Unregister(Box::new(
            maintenance::Mutation::Unregister(item),
        ))),
        removal::Intent::Cleanup(item) => Some(RemovalAction::CleanupPreview(
            cleanup.open(item.clone()),
            item,
        )),
        removal::Intent::None | removal::Intent::Cancel => None,
    }
}

/// Route every action against the selected row's identity and capabilities.
fn navigate(
    key: impl Into<KeyEvent>,
    state: &AppState,
    scroll: &mut u16,
    interaction: &mut interaction::Interaction,
) -> Option<String> {
    let key = key.into();
    if interaction.draft.is_some() || interaction.sending || key.kind == KeyEventKind::Release {
        return None;
    }
    if key
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
    {
        return None;
    }
    let items = state.items();
    if interaction.response_bar.is_some() {
        interaction.response_key(key, items);
        return None;
    }
    match key.code {
        KeyCode::Tab if key.kind == KeyEventKind::Press && key.modifiers.is_empty() => {
            interaction.show_sessions = !interaction.show_sessions;
            state.sync_selection(interaction);
            interaction.reveal_selection = true;
            *scroll = 0;
        }
        KeyCode::Down | KeyCode::Char('j') => state.move_row(interaction, 1),
        KeyCode::Up | KeyCode::Char('k') => state.move_row(interaction, -1),
        KeyCode::Enter | KeyCode::Char('r') if interaction.selected_pane.is_some() => {
            // Recheck visibility and discovery rather than registering a stale pane.
            let rows = state.rows(interaction.show_sessions);
            interaction.registration_requested = rows.iter().find_map(|row| match row {
                listing::Row::Session { pane, .. } if row.selected(interaction) => {
                    Some(pane.id.clone())
                }
                _ => None,
            });
            if interaction.registration_requested.is_none() {
                interaction.message =
                    Some("Session unavailable. Wait for discovery and try again.".into());
            }
        }
        KeyCode::Enter => {
            if let Some(selected) = interaction.selected(items) {
                return Some(selected.item.id.clone());
            }
            interaction.message = Some("Select an item in WORK first.".into());
        }
        KeyCode::Char('r') if key.kind == KeyEventKind::Press && key.modifiers.is_empty() => {
            interaction.begin_response(items);
        }
        KeyCode::Char('c')
            if interaction.selected_pane.is_none()
                && key.kind == KeyEventKind::Press
                && key.modifiers.is_empty() =>
        {
            interaction.removal_requested = interaction
                .selected(items)
                .map(|selected| selected.item.clone());
            interaction.message = interaction
                .removal_requested
                .is_none()
                .then(|| "Select an item in WORK first.".into());
        }
        KeyCode::Char('e') if interaction.selected_pane.is_none() => {
            interaction.maintenance_requested = interaction
                .selected(items)
                .map(|selected| maintenance::Request::Edit(selected.item.clone()));
            interaction.message = interaction
                .maintenance_requested
                .is_none()
                .then(|| "Select an item in WORK first.".into());
        }
        KeyCode::Char('d') if interaction.selected_pane.is_none() => {
            interaction.review_requested = interaction
                .selected(items)
                .map(|selected| selected.item.id.clone());
            interaction.message = interaction
                .review_requested
                .is_none()
                .then(|| "Select an item in WORK first.".into());
        }
        _ => {}
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use workbench_core::{
        AgentStatus, ApprovalDecision, ApprovalRequest, PaneAvailability, WorkItem, WorkItemKind,
    };

    #[test]
    fn test_switch_state() {
        let mut interaction = interaction::Interaction {
            selected_id: Some("task".into()),
            detail_item_id: Some("task".into()),
            work_list_offset: 7,
            message: Some("stale notice".into()),
            ..Default::default()
        };
        assert!(finish_switch("task", &mut interaction, Ok(true)).is_none());
        assert_eq!(interaction.selected_id.as_deref(), Some("task"));
        assert_eq!(interaction.detail_item_id.as_deref(), Some("task"));
        assert_eq!(interaction.work_list_offset, 7);
        assert!(interaction.message.is_none());
    }

    #[test]
    fn test_switch_failure() {
        let mut interaction = interaction::Interaction {
            selected_id: Some("task".into()),
            ..Default::default()
        };
        for error in [
            ActionError::MissingPane {
                item: "task".into(),
                pane: "%14".into(),
            },
            ActionError::Transport(workbench_core::DiscoveryError::CommandFailed {
                code: Some(1),
                message: "client disappeared".into(),
            }),
        ] {
            assert!(finish_switch("task", &mut interaction, Err(error)).is_none());
            assert!(
                interaction
                    .message
                    .as_deref()
                    .unwrap()
                    .starts_with("Could not open task:")
            );
            assert_eq!(interaction.selected_id.as_deref(), Some("task"));
        }
    }

    #[test]
    fn test_attach_exit() {
        let mut interaction = interaction::Interaction::default();
        assert!(matches!(finish_switch("task", &mut interaction, Ok(false)),
            Some(RunExit::Focus(id)) if id == "task"));
        assert!(interaction.message.is_none());
    }

    fn approval_state() -> AppState {
        let mut state = state(&[
            ("waiting", AgentStatus::WaitingForInput),
            ("finished", AgentStatus::Complete),
        ]);
        if let Some(Ok(items)) = &mut state.work_items {
            items[0].input_prompt = Some("Would you like to run the following command?\n$ cargo test\n\nOptions:\n› 1. Yes, proceed (y)\n  2. No, and tell Codex what to do differently (esc)".into());
        }
        state
    }

    #[test]
    fn test_approval_keys() {
        let state = approval_state();
        for code in ['y', 'n'] {
            let mut interaction = interaction::Interaction::default();
            interaction.sync(state.items());
            navigate(KeyCode::Char(code), &state, &mut 0, &mut interaction);
            assert!(interaction.approval_requested.is_none());
            navigate(KeyCode::Char('r'), &state, &mut 0, &mut interaction);
            navigate(KeyCode::Char(code), &state, &mut 0, &mut interaction);

            let request = interaction.approval_requested.take().unwrap();
            assert_eq!(request.item_id(), "waiting");
            assert_eq!(
                request.decision(),
                if code == 'y' {
                    ApprovalDecision::ApproveOnce
                } else {
                    ApprovalDecision::RejectAndReply
                }
            );

            let mut interaction = interaction::Interaction::default();
            interaction.sync(state.items());
            interaction.begin_response(state.items());
            for modifiers in [
                KeyModifiers::CONTROL,
                KeyModifiers::ALT,
                KeyModifiers::SUPER,
                KeyModifiers::SHIFT,
            ] {
                navigate(
                    KeyEvent::new(KeyCode::Char(code), modifiers),
                    &state,
                    &mut 0,
                    &mut interaction,
                );
                assert!(interaction.approval_requested.is_none());
            }
            for kind in [KeyEventKind::Repeat, KeyEventKind::Release] {
                navigate(
                    KeyEvent::new_with_kind(KeyCode::Char(code), KeyModifiers::NONE, kind),
                    &state,
                    &mut 0,
                    &mut interaction,
                );
                assert!(interaction.approval_requested.is_none());
            }
            interaction.sending = true;
            navigate(KeyCode::Char(code), &state, &mut 0, &mut interaction);
            assert!(interaction.approval_requested.is_none());
            interaction.sending = false;
            navigate(KeyCode::Esc, &state, &mut 0, &mut interaction);
            assert!(interaction.response_bar.is_none());
            interaction.begin_reply(state.items());
            navigate(KeyCode::Char(code), &state, &mut 0, &mut interaction);
            assert!(interaction.approval_requested.is_none());
            interaction
                .draft
                .as_mut()
                .unwrap()
                .edit(KeyCode::Char(code).into())
                .unwrap();
            assert_eq!(interaction.draft.as_ref().unwrap().text, code.to_string());
            interaction.draft = None;
            interaction.selected_id = Some("finished".into());
            navigate(KeyCode::Char(code), &state, &mut 0, &mut interaction);
            assert!(interaction.approval_requested.is_none());
            navigate(
                KeyCode::Char(code),
                &AppState::default(),
                &mut 0,
                &mut interaction,
            );
            assert!(interaction.approval_requested.is_none());
        }
    }

    #[test]
    fn test_captured_rejection() {
        let state = approval_state();
        let mut interaction = interaction::Interaction::default();
        interaction.sync(state.items());
        let request =
            ApprovalRequest::capture(&state.items()[0], ApprovalDecision::RejectAndReply).unwrap();
        interaction.sending = true;
        interaction.selected_id = Some("finished".into());
        interaction.finish_approval(&request, Ok(()));
        assert!(!interaction.sending);
        let draft = interaction.draft.as_ref().unwrap();
        assert_eq!(draft.item_id, "waiting");
        assert!(draft.text.is_empty());
        interaction.draft = None;
        interaction.finish_approval(&request, Err(workbench_core::ApprovalError::Changed));
        assert!(interaction.draft.is_none());
        assert!(
            interaction
                .message
                .as_ref()
                .unwrap()
                .contains("Not retried")
        );
        let request =
            ApprovalRequest::capture(&state.items()[0], ApprovalDecision::ApproveOnce).unwrap();
        interaction.finish_approval(&request, Ok(()));
        assert!(interaction.draft.is_none());
        assert!(interaction.message.is_none());
        let feedback = interaction.approval_feedback.as_ref().unwrap();
        assert_eq!(feedback.item_id, "waiting");
        assert_eq!(feedback.label(), "✓ Approval sent");
    }

    #[test]
    fn test_response_keys() {
        let state = approval_state();
        let mut interaction = interaction::Interaction::default();
        interaction.sync(state.items());

        let mut scroll = 4;
        navigate(KeyCode::Char('r'), &state, &mut scroll, &mut interaction);
        assert!(interaction.response_bar.is_some());
        for key in [
            KeyCode::Char('j'),
            KeyCode::Down,
            KeyCode::Char('e'),
            KeyCode::Char('u'),
            KeyCode::Char('c'),
            KeyCode::Char('s'),
            KeyCode::Char('q'),
            KeyCode::Tab,
        ] {
            assert!(navigate(key, &state, &mut scroll, &mut interaction).is_none());
            assert_eq!(interaction.selected_id.as_deref(), Some("waiting"));

            assert!(interaction.maintenance_requested.is_none());
            assert!(interaction.removal_requested.is_none());
        }
        navigate(
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
            &state,
            &mut scroll,
            &mut interaction,
        );
        assert_eq!(scroll, 4);
        navigate(KeyCode::Enter, &state, &mut scroll, &mut interaction);
        assert!(interaction.response_bar.is_none());
        assert_eq!(interaction.draft.as_ref().unwrap().item_id, "waiting");
        assert!(interaction.approval_requested.is_none());
    }

    #[test]
    fn test_rename_selection() {
        let directory = tempfile::tempdir().unwrap();
        let engine = Engine::new(directory.path().join("items.json"));
        let mut state = state(&[("A", AgentStatus::Complete)]);
        if let Some(Ok(items)) = &mut state.work_items {
            items[0].item.workspace = directory.path().into();
        }
        let original = state.items()[0].item.clone();
        engine.register_work_item(original.clone()).unwrap();
        state.discovery = Some(Ok(Snapshot {
            sessions: vec![workbench_core::Session {
                id: "$1".into(),
                name: "main".into(),
                windows: vec![workbench_core::Window {
                    id: "@1".into(),
                    index: 0,
                    name: "agent".into(),
                    panes: vec![workbench_core::Pane {
                        id: "%14".into(),
                        index: 0,
                        title: "Agent".into(),
                        current_command: Some("codex".into()),
                        working_directory: Some("/work".into()),
                    }],
                }],
            }],
        }));
        let mut interaction = interaction::Interaction::default();
        interaction.sync(state.items());

        let mutation = maintenance::Mutation::Details {
            expected: original.clone(),
            id: "New λ🙂".into(),
            description: "Updated description".into(),
        };
        mutation.execute(&engine).unwrap();
        state.apply_detail_edit(&mutation, &mut interaction);
        state.reload_registry(&engine);
        interaction.sync(state.items());
        assert_eq!(interaction.selected_id.as_deref(), Some("New λ🙂"));
        assert_eq!(state.items()[0].status, AgentStatus::Complete);
        assert_eq!(state.items()[0].item.title, "Updated description");
    }

    #[test]
    fn test_edit_selection() {
        let state = state(&[
            ("hidden", AgentStatus::Running),
            ("A", AgentStatus::Complete),
        ]);

        let mut interaction = interaction::Interaction::default();
        interaction.sync(state.items());
        for code in [KeyCode::Char('e'), KeyCode::Char('n'), KeyCode::Char('u')] {
            navigate(code, &state, &mut 0, &mut interaction);
            if code == KeyCode::Char('e') {
                match interaction.maintenance_requested.take().unwrap() {
                    maintenance::Request::Edit(item) => {
                        assert_eq!(code, KeyCode::Char('e'));
                        assert_eq!(item.id, "hidden");
                    }
                }
            } else {
                assert!(interaction.maintenance_requested.is_none());
            }
        }

        let mut interaction = interaction::Interaction::default();
        navigate(
            KeyCode::Char('e'),
            &AppState::default(),
            &mut 0,
            &mut interaction,
        );
        assert!(interaction.maintenance_requested.is_none());
        assert!(interaction.message.unwrap().contains("Select an item"));
        let mut interaction = interaction::Interaction::default();
        interaction.sync(state.items());
        for (code, modifier) in [
            ('e', KeyModifiers::CONTROL),
            ('n', KeyModifiers::CONTROL),
            ('u', KeyModifiers::ALT),
        ] {
            navigate(
                KeyEvent::new(KeyCode::Char(code), modifier),
                &state,
                &mut 0,
                &mut interaction,
            );
            assert!(interaction.maintenance_requested.is_none());
        }
        interaction.begin_reply(state.items());
        navigate(KeyCode::Char('u'), &state, &mut 0, &mut interaction);
        assert!(interaction.maintenance_requested.is_none());
    }

    #[test]
    fn test_removal_key() {
        let state = state(&[("A", AgentStatus::Running), ("B", AgentStatus::Complete)]);

        let mut interaction = interaction::Interaction {
            selected_id: Some("B".into()),
            ..Default::default()
        };
        navigate(KeyCode::Char('c'), &state, &mut 0, &mut interaction);

        assert_eq!(
            interaction.removal_requested.take(),
            Some(state.items()[1].item.clone())
        );

        let mut interaction = interaction::Interaction::default();
        interaction.sync(state.items());
        for modifier in [
            KeyModifiers::CONTROL,
            KeyModifiers::ALT,
            KeyModifiers::SUPER,
            KeyModifiers::SHIFT,
        ] {
            navigate(
                KeyEvent::new(KeyCode::Char('c'), modifier),
                &state,
                &mut 0,
                &mut interaction,
            );
            assert!(interaction.removal_requested.is_none());
        }
        for kind in [KeyEventKind::Repeat, KeyEventKind::Release] {
            let mut key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE);
            key.kind = kind;
            navigate(key, &state, &mut 0, &mut interaction);
            assert!(interaction.removal_requested.is_none());
        }
        interaction.begin_reply(state.items());
        navigate(KeyCode::Char('c'), &state, &mut 0, &mut interaction);
        assert!(interaction.removal_requested.is_none());
        interaction.draft = None;
        navigate(
            KeyCode::Char('c'),
            &AppState::default(),
            &mut 0,
            &mut interaction,
        );
        assert!(interaction.removal_requested.is_none());
        assert!(
            interaction
                .message
                .as_ref()
                .unwrap()
                .contains("Select an item")
        );
    }

    #[test]
    fn test_removal_actions() {
        let directory = tempfile::tempdir().unwrap();
        let engine = Engine::new(directory.path().join("items.json"));
        let state = state(&[("A", AgentStatus::Running), ("B", AgentStatus::Complete)]);
        for item in state.items() {
            engine.register_work_item(item.item.clone()).unwrap();
        }
        let saved = engine.work_items().unwrap();
        for code in ['c', 'u'] {
            let mut interaction = interaction::Interaction {
                selected_id: Some("B".into()),
                ..Default::default()
            };

            let mut scroll = 5;
            navigate(KeyCode::Char('c'), &state, &mut scroll, &mut interaction);
            assert!(interaction.maintenance_requested.is_none());
            let mut menu = removal::RemovalUi::default();
            menu.open(interaction.removal_requested.take().unwrap());
            interaction.selected_id = Some("A".into());
            menu.observe(Some(state.items()));
            let maintenance = maintenance::MaintenanceUi::default();
            let mut cleanup = cleanup::CleanupUi::default();
            let action = open_removal_action(menu.key(KeyCode::Char(code).into()), &mut cleanup);
            assert_eq!(engine.work_items().unwrap(), saved);

            assert_eq!(scroll, 5);
            if code == 'u' {
                let Some(RemovalAction::Unregister(mutation)) = action else {
                    panic!("u must emit unregister directly without another confirmation")
                };
                assert!(menu.is_saving());
                assert!(!maintenance.is_open());
                assert!(!cleanup.is_open());
                assert!(
                    matches!(mutation.as_ref(), maintenance::Mutation::Unregister(item) if item.id == "B")
                );
                menu.finish_unregister(mutation.execute(&engine));
                assert!(interaction.message.is_none());
                assert!(!menu.is_open());
                assert_eq!(engine.work_items().unwrap(), [saved[0].clone()]);
            } else {
                let Some(RemovalAction::CleanupPreview(ticket, target)) = action else {
                    panic!("c must open cleanup preview, never unregister")
                };
                assert!(!menu.is_open());
                assert_eq!(target, state.items()[1].item);
                assert!(cleanup.is_open());
                assert!(!maintenance.is_open());
                cleanup.finish_preview(ticket, Err("Dirty workspace; cleanup blocked.".into()));
                assert!(matches!(
                    cleanup.key(KeyCode::Enter.into()),
                    cleanup::Intent::Cancel
                ));
                assert!(!maintenance.is_open());
                assert_eq!(engine.work_items().unwrap(), saved);
            }
        }
    }

    #[test]
    fn test_removal_layout() {
        use ratatui::{Terminal, backend::TestBackend};
        let state = state(&[("A", AgentStatus::Running), ("B", AgentStatus::Complete)]);
        let mut interaction = interaction::Interaction {
            selected_id: Some("B".into()),
            ..Default::default()
        };
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal
            .draw(|frame| ui::render_work(frame, &state, &mut 0, &mut interaction))
            .unwrap();
        let baseline = terminal.backend().buffer().clone();
        let mut menu = removal::RemovalUi::default();
        menu.open(state.items()[1].item.clone());
        terminal
            .draw(|frame| {
                ui::render_work(frame, &state, &mut 0, &mut interaction);
                menu.render(frame);
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        // A 58x8 centered popup: outside it, only the context footer changes.
        for y in 0..23 {
            for x in 0..100 {
                if !(21..79).contains(&x) || !(8..16).contains(&y) {
                    assert_eq!(
                        buffer[(x, y)],
                        baseline[(x, y)],
                        "changed background at {x},{y}"
                    );
                }
            }
        }
        let footer: String = buffer.content()[2300..2400]
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(
            footer.trim().is_empty(),
            "bottom help bar still visible: {footer}"
        );
        assert_eq!(interaction.selected_id.as_deref(), Some("B"));
        assert_eq!(interaction.work_list_offset, 0);
        assert_eq!(interaction.detail_item_id.as_deref(), Some("B"));
        theme::assert_text_style(buffer, "Clean Up Options", theme::TEAL, theme::MANTLE);
        assert!(matches!(
            menu.key(KeyCode::Esc.into()),
            removal::Intent::Cancel
        ));
        terminal
            .draw(|frame| {
                ui::render_work(frame, &state, &mut 0, &mut interaction);
                menu.render(frame);
            })
            .unwrap();
        assert_eq!(terminal.backend().buffer(), &baseline);
    }

    #[test]
    fn test_unregister_recheck() {
        let directory = tempfile::tempdir().unwrap();
        let engine = Engine::new(directory.path().join("items.json"));
        let mut original = state(&[("A", AgentStatus::Running)]).items()[0]
            .item
            .clone();
        original.workspace = directory.path().join("workspace");
        std::fs::create_dir(&original.workspace).unwrap();
        let source = original.workspace.join("source.rs");
        std::fs::write(&source, "keep this content").unwrap();
        engine.register_work_item(original.clone()).unwrap();
        let mut menu = removal::RemovalUi::default();
        let mut cleanup = cleanup::CleanupUi::default();
        menu.open(original.clone());
        let Some(RemovalAction::Unregister(mutation)) =
            open_removal_action(menu.key(KeyCode::Char('u').into()), &mut cleanup)
        else {
            panic!("expected direct unregister")
        };
        let updated = engine
            .update_work_item_description(&original, "Changed externally")
            .unwrap();
        let error = mutation.execute(&engine).unwrap_err();
        assert!(error.contains("changed since"));
        menu.finish_unregister(Err(error));
        assert!(menu.is_open());
        assert!(!menu.is_saving());
        assert!(!cleanup.is_open());
        assert_eq!(engine.work_items().unwrap(), std::slice::from_ref(&updated));
        assert_eq!(
            std::fs::read_to_string(&source).unwrap(),
            "keep this content"
        );
        assert!(matches!(
            menu.key(KeyCode::Esc.into()),
            removal::Intent::Cancel
        ));
        menu.open(updated.clone());
        let Some(RemovalAction::Unregister(mutation)) =
            open_removal_action(menu.key(KeyCode::Char('u').into()), &mut cleanup)
        else {
            panic!("expected explicit retry against the refreshed target")
        };
        menu.finish_unregister(mutation.execute(&engine));
        assert!(!menu.is_open());
        assert!(engine.work_items().unwrap().is_empty());
        assert_eq!(
            std::fs::read_to_string(&source).unwrap(),
            "keep this content"
        );
    }

    #[test]
    fn test_registry_refresh() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("items.json");
        let engine = Engine::new(&path);
        let mut stale = state(&[("A", AgentStatus::Running), ("B", AgentStatus::Complete)]);
        let workspace = directory.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        if let Some(Ok(items)) = &mut stale.work_items {
            for item in items {
                item.item.workspace = workspace.clone();
            }
        }
        let a = stale.items()[0].item.clone();
        let b = stale.items()[1].item.clone();
        engine.register_work_item(a.clone()).unwrap();
        engine.register_work_item(b.clone()).unwrap();
        stale.discovery = Some(Ok(Snapshot {
            sessions: vec![workbench_core::Session {
                id: "$1".into(),
                name: "main".into(),
                windows: vec![workbench_core::Window {
                    id: "@1".into(),
                    index: 0,
                    name: "agent".into(),
                    panes: vec![workbench_core::Pane {
                        id: "%14".into(),
                        index: 0,
                        title: "Agent".into(),
                        current_command: Some("codex".into()),
                        working_directory: Some("/work".into()),
                    }],
                }],
            }],
        }));
        let updated = engine
            .update_work_item_description(&a, "New description")
            .unwrap();
        let mut fresh = stale.clone();
        fresh.reload_registry(&engine);
        assert_eq!(fresh.items()[0].item, updated);
        assert_eq!(fresh.items()[0].status, AgentStatus::Running);
        std::fs::remove_dir(&workspace).unwrap();
        fresh.reload_registry(&engine);
        assert_eq!(fresh.items()[0].status, AgentStatus::Running);
        assert_eq!(
            fresh.items()[0].resource_issue(),
            Some(workbench_core::ResourceIssue::WorkspaceMissing)
        );
        std::fs::create_dir(&workspace).unwrap();
        fresh.reload_registry(&engine);
        assert!(fresh.items()[0].resource_issue().is_none());
        assert_eq!(fresh.items()[0].status, AgentStatus::Running);
        engine.unregister_work_item(&b).unwrap();
        let mut pending_refresh = stale;
        pending_refresh.reload_registry(&engine);
        assert_eq!(pending_refresh.items().len(), 1);
        assert_eq!(pending_refresh.items()[0].item.title, "New description");
        engine.unregister_work_item(&updated).unwrap();
        let mut replacement = updated;
        replacement.pane_id = "%99".into();
        engine.register_work_item(replacement).unwrap();
        pending_refresh.reload_registry(&engine);
        assert_eq!(pending_refresh.items()[0].status, AgentStatus::Unknown);
        std::fs::write(&path, "{broken").unwrap();
        pending_refresh.reload_registry(&engine);
        assert!(matches!(pending_refresh.work_items, Some(Err(_))));
    }

    fn state(items: &[(&str, AgentStatus)]) -> AppState {
        AppState::from_refresh(
            Ok(Snapshot::default()),
            Ok(items
                .iter()
                .map(|(id, status)| WorkItemState {
                    item: WorkItem {
                        id: (*id).into(),
                        title: "Task".into(),
                        repository: "/work".into(),
                        workspace: "/work".into(),
                        branch: None,
                        kind: WorkItemKind::Implementation,
                        pane_id: "%14".into(),
                    },
                    status: *status,
                    pane: PaneAvailability::Present,
                    workspace_availability: workbench_core::WorkspaceAvailability::Present,
                    status_detail: "Observed locally.".into(),
                    input_prompt: None,
                })
                .collect()),
        )
    }

    #[test]
    fn test_scroll_keys() {
        let state = state(&[("A", AgentStatus::WaitingForInput)]);

        let mut interaction = interaction::Interaction::default();
        interaction.sync(state.items());
        let selected = interaction.selected_id.clone();
        let mut scroll = 7;
        let reveal_selection = interaction.reveal_selection;
        for key in ['d', 'd', 'u', 'u', 'u'] {
            assert!(
                navigate(
                    KeyEvent::new(KeyCode::Char(key), KeyModifiers::CONTROL),
                    &state,
                    &mut scroll,
                    &mut interaction
                )
                .is_none()
            );
            assert_eq!(scroll, 7);

            assert_eq!(interaction.selected_id, selected);
            assert!(interaction.message.is_none());
            assert!(interaction.draft.is_none());
            assert_eq!(interaction.reveal_selection, reveal_selection);
        }
        scroll = 7;
        navigate(KeyCode::PageDown, &state, &mut scroll, &mut interaction);
        assert_eq!(scroll, 7);
        navigate(KeyCode::PageUp, &state, &mut scroll, &mut interaction);
        assert_eq!(scroll, 7);
        for code in [KeyCode::Home, KeyCode::F(1)] {
            navigate(code, &state, &mut scroll, &mut interaction);
            assert_eq!(scroll, 7);
            assert_eq!(interaction.selected_id, selected);
        }
    }

    #[test]
    fn test_empty_scroll_keys() {
        let state = state(&[]);

        let mut interaction = interaction::Interaction::default();
        for initial_offset in [0, u16::MAX] {
            let mut scroll = initial_offset;
            for code in ['d', 'u'] {
                navigate(
                    KeyEvent::new(KeyCode::Char(code), KeyModifiers::CONTROL),
                    &state,
                    &mut scroll,
                    &mut interaction,
                );
                assert_eq!(scroll, initial_offset);

                assert!(interaction.review_requested.is_none());
                assert!(interaction.removal_requested.is_none());
            }
        }
    }

    #[test]
    fn test_modified_keys() {
        let state = state(&[("A", AgentStatus::WaitingForInput)]);

        let mut interaction = interaction::Interaction::default();
        interaction.sync(state.items());
        let mut scroll = 10;
        for key in [
            KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL),
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::ALT),
            KeyEvent::new(
                KeyCode::Char('d'),
                KeyModifiers::CONTROL | KeyModifiers::ALT,
            ),
        ] {
            navigate(key, &state, &mut scroll, &mut interaction);
            assert_eq!(scroll, 10);
            assert!(interaction.message.is_none());
            assert!(interaction.draft.is_none());
        }
        interaction.begin_reply(state.items());
        interaction.draft.as_mut().unwrap().insert("yes").unwrap();
        for ch in ['d', 'u'] {
            navigate(
                KeyEvent::new(KeyCode::Char(ch), KeyModifiers::CONTROL),
                &state,
                &mut scroll,
                &mut interaction,
            );
            assert_eq!(scroll, 10);
            assert_eq!(interaction.draft.as_ref().unwrap().text, "yes");
        }
    }

    #[test]
    fn test_refresh_selection() {
        let initial = state(&[
            ("A", AgentStatus::WaitingForInput),
            ("B", AgentStatus::Complete),
        ]);
        let mut interaction = interaction::Interaction::default();
        interaction.sync(initial.items());
        interaction.begin_reply(initial.items());
        let next = state(&[("A", AgentStatus::Running), ("B", AgentStatus::Complete)]);
        next.sync_selection(&mut interaction);
        assert_eq!(interaction.selected_id.as_deref(), Some("A"));
        assert_eq!(interaction.draft.as_ref().unwrap().item_id, "A");
        let active = state(&[("A", AgentStatus::Running), ("B", AgentStatus::Unknown)]);
        active.sync_selection(&mut interaction);
        assert_eq!(interaction.selected_id.as_deref(), Some("A"));
        assert_eq!(active.items().len(), 2);
        let failed = AppState::from_refresh(
            Err("tmux unavailable".into()),
            Err("state unreadable".into()),
        );
        failed.sync_selection(&mut interaction);
        assert_eq!(interaction.selected_id.as_deref(), Some("A"));
        let removed = state(&[("B", AgentStatus::Unknown)]);
        removed.sync_selection(&mut interaction);
        assert_eq!(interaction.selected_id.as_deref(), Some("B"));
        let empty = state(&[]);
        empty.sync_selection(&mut interaction);
        assert!(interaction.selected_id.is_none());
        assert_eq!(interaction.draft.as_ref().unwrap().item_id, "A");
    }

    #[test]
    fn test_missing_selection() {
        let empty = state(&[]);
        let failed = AppState::from_refresh(
            Err("tmux unavailable".into()),
            Err("state unreadable".into()),
        );
        for state in [empty, failed] {
            let mut scroll = 0;
            let mut interaction = interaction::Interaction {
                selected_id: Some("hidden".into()),
                ..Default::default()
            };
            assert!(navigate(KeyCode::Enter, &state, &mut scroll, &mut interaction).is_none());
            navigate(KeyCode::Char('r'), &state, &mut scroll, &mut interaction);
            assert!(interaction.draft.is_none());
            navigate(KeyCode::Char('d'), &state, &mut scroll, &mut interaction);
            assert!(interaction.review_requested.is_none());
        }
    }
}
