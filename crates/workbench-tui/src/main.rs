use std::{io, sync::Arc, time::Duration};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use tokio::{
    sync::watch,
    task::JoinSet,
    time::{self, MissedTickBehavior},
};
use workbench_core::{Engine, Snapshot, WorkItemState, attention_items, validate_agent_input};

mod cli;
mod help;
mod interaction;
mod maintenance;
mod registration;
mod review;
mod theme;
mod ui;

type DiscoveryState = Option<Result<Snapshot, String>>;

#[derive(Clone, Debug, Default)]
struct AppState {
    discovery: DiscoveryState,
    work_items: Option<Result<Vec<WorkItemState>, String>>,
    attention: Vec<WorkItemState>,
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
            let mut updated = expected.clone();
            updated.id.clone_from(id);
            updated.title.clone_from(description);
            if let Some(Ok(items)) = &mut self.work_items {
                for state in items {
                    if state.item == *expected {
                        state.item = updated.clone();
                    }
                }
            }
            interaction
                .attention_tracker
                .rename_work_item(expected, &updated);
            if interaction.selected_id.as_ref() == Some(&expected.id) {
                interaction.selected_id = Some(id.clone());
            }
        }
    }
    fn apply_acknowledgement(&mut self, interaction: &mut interaction::Interaction) {
        if let Some(target) = interaction.acknowledgement_requested.take() {
            match interaction.attention_tracker.acknowledge(&target) {
                Ok(()) => {
                    self.attention = interaction.attention_tracker.items(self.items());
                    interaction.sync(&self.attention);
                    interaction.reveal_selection = true;
                    interaction.message =
                        Some("Turn acknowledged; work item and review state unchanged.".into());
                }
                Err(error) => interaction.message = Some(error.to_string()),
            }
        }
    }
    fn refresh_attention(&mut self, tracker: &mut workbench_core::AttentionTracker) {
        self.attention = match &self.work_items {
            Some(Ok(items)) => tracker.observe(items),
            _ => Vec::new(), // Load errors are not evidence that registrations were removed.
        };
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
                            current.attention_prompt.clone_from(&old.attention_prompt);
                            current.completion_fingerprint = old.completion_fingerprint;
                        }
                    }
                    items
                })
                .map_err(|error| error.to_string()),
        );
        self.attention = attention_items(self.items());
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
        let attention = work_items
            .as_ref()
            .map(|items| attention_items(items))
            .unwrap_or_default();
        Self {
            discovery: Some(discovery),
            work_items: Some(work_items),
            attention,
        }
    }

    fn items(&self) -> &[WorkItemState] {
        self.work_items
            .as_ref()
            .and_then(|items| items.as_ref().ok())
            .map_or(&[], Vec::as_slice)
    }

    fn items_for(&self, view: ui::View) -> &[WorkItemState] {
        match view {
            ui::View::Attention => &self.attention,
            _ => self.items(),
        }
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
    }
    let mut interaction = interaction::Interaction::default();
    let mut view = ui::View::default();
    let mut reviews = review::ReviewUi::new(options.diff_base);
    loop {
        let mut terminal = init_terminal()?;
        let result = run(
            &mut terminal,
            Arc::clone(&engine),
            &mut interaction,
            &mut view,
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
    view: &mut ui::View,
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
    let result = event_loop(terminal, &mut receiver, engine, interaction, view, reviews).await;
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
    view: &mut ui::View,
    reviews: &mut review::ReviewUi,
) -> io::Result<RunExit> {
    let mut state = AppState::default();
    let mut scroll = 0_u16;
    let mut redraw = true;
    let mut sends = JoinSet::new();
    let mut diffs = JoinSet::new();
    let mut review_saves = JoinSet::new();
    let mut preparations = JoinSet::new();
    let mut registrations = JoinSet::<Result<workbench_core::WorkItem, String>>::new();
    let mut registration = registration::RegistrationUi::default();
    let mut maintenance = maintenance::MaintenanceUi::default();
    let mut help = help::HelpUi::default();
    let mut mutations = JoinSet::<(Box<maintenance::Mutation>, Result<(), String>)>::new();
    let mut input_tick = time::interval(Duration::from_millis(100));
    input_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        if redraw {
            terminal.draw(|frame| {
                if help.is_open() {
                    help.render(frame);
                } else if maintenance.is_open() {
                    maintenance.render(frame);
                } else if registration.pane.is_some() {
                    registration.render(frame);
                } else if reviews.is_open() {
                    reviews.render(frame);
                } else {
                    match *view {
                        ui::View::Work => ui::render_work(frame, &state, &mut scroll, interaction),
                        ui::View::Attention => {
                            ui::render_attention(frame, &state, &mut scroll, interaction)
                        }
                        ui::View::Sessions => {
                            ui::render_sessions(frame, &state, &mut scroll, interaction)
                        }
                    }
                }
            })?;
            redraw = false;
        }
        tokio::select! {
            completed = mutations.join_next(), if !mutations.is_empty() => {
                match completed {
                    Some(Ok((mutation, result))) => {
                        let success = result.is_ok();
                        maintenance.finish(result);
                        if success {
                            state.apply_detail_edit(&mutation, interaction);
                            state.reload_registry(&engine);
                            state.refresh_attention(&mut interaction.attention_tracker);
                            interaction.sync(state.items_for(*view));
                            interaction.reveal_selection = true;
                            interaction.message = Some(mutation.success_message());
                        }
                    }
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
                        state.refresh_attention(&mut interaction.attention_tracker);
                        *view = ui::View::Work;
                        scroll = 0;
                        interaction.selected_id = Some(item.id.clone());
                        interaction.reveal_selection = true;
                        interaction.message = Some(format!("Registered {} from pane {}.", item.id, item.pane_id));
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
            completed = review_saves.join_next(), if !review_saves.is_empty() => {
                match completed {
                    Some(Ok((ticket, result))) => reviews.finish_save(ticket, result),
                    _ => reviews.fail_saving("Review save task stopped; reopen review to verify the saved state before retrying.".into()),
                }
                redraw = true;
            }
            // Apply a successful local rename before its watcher snapshot can
            // prune the old ID's session acknowledgement. The watcher continues
            // running; consume its latest snapshot immediately after the save.
            changed = receiver.changed(), if mutations.is_empty() => {
                changed.map_err(|_| io::Error::other("Discovery task stopped unexpectedly"))?;
                let previous_position = state.items_for(*view).iter().position(|item| Some(&item.item.id) == interaction.selected_id.as_ref());
                let updated = receiver.borrow_and_update().clone();
                let previous_attention = state.attention.clone();
                state = updated;
                state.reload_registry(&engine);
                state.refresh_attention(&mut interaction.attention_tracker);
                let attention_changed = previous_attention != state.attention;
                if let Some(snapshot) = state.snapshot() { interaction.sync_panes(snapshot); }
                interaction.sync(state.items_for(*view));
                let position = state.items_for(*view).iter().position(|item| Some(&item.item.id) == interaction.selected_id.as_ref());
                interaction.reveal_selection |= position != previous_position || (*view == ui::View::Attention && attention_changed);
                redraw = true;
            }
            completed = sends.join_next(), if !sends.is_empty() => {
                interaction.sending = false;
                match completed {
                    Some(Ok((id, result))) => match result {
                        Ok(()) => { interaction.message = Some(format!("Reply submitted to {id}.")); interaction.draft = None; }
                        Err(error) => interaction.message = Some(format!("Reply to {id} failed: {error}. Not retried; check the pane before resending.")),
                    },
                    _ => interaction.message = Some("Input task stopped unexpectedly. Check the pane before resending.".into()),
                }
                redraw = true;
            }
            _ = input_tick.tick() => {
                while event::poll(Duration::ZERO)? {
                    redraw = true;
                    let event = event::read()?;
                    if matches!(event, Event::Resize(_, _)) { interaction.reveal_selection = true; interaction.reveal_pane = true; }
                    let context = maintenance.help_context().unwrap_or_else(|| {
                        if registration.pane.is_some() { help::Context::Registration }
                        else if reviews.is_open() { help::Context::Review }
                        else if interaction.draft.is_some() { help::Context::Reply }
                        else { help::Context::view(*view, interaction.show_all_panes) }
                    });
                    if help.event(&event, context, terminal.size()?.height) {
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
                        && let Err(error) = draft.append(text)
                    {
                        interaction.message = Some(error);
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
                                        Err(error) => interaction.message = Some(error.to_string()),
                                        Ok(()) => {
                                            let id = draft.item_id.clone();
                                            let text = draft.text.clone();
                                            let engine = Arc::clone(&engine);
                                            interaction.sending = true;
                                            interaction.message = Some(format!("Sending reply to {id}…"));
                                            sends.spawn(async move { let result = engine.send_agent_input(&id, &text).await; (id, result) });
                                        }
                                    }
                                }
                                _ if !interaction.sending => if let Err(error) = draft.edit(key) { interaction.message = Some(error); },
                                _ => {}
                            }
                            continue;
                        }
                        match key.code {
                            KeyCode::Char('q') | KeyCode::Esc => return Ok(RunExit::Quit),
                            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return Ok(RunExit::Quit),
                            _ => {
                                if let Some(id) = navigate(key, &state, view, &mut scroll, interaction, terminal.size()?.height) { return Ok(RunExit::Focus(id)); }
                                state.apply_acknowledgement(interaction);
                                if let Some(request) = interaction.maintenance_requested.take() {
                                    interaction.message = None;
                                    maintenance.open(request);
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

/// Route list actions against the visible subset, never the hidden WORK list.
fn navigate(
    key: impl Into<KeyEvent>,
    state: &AppState,
    view: &mut ui::View,
    scroll: &mut u16,
    interaction: &mut interaction::Interaction,
    height: u16,
) -> Option<String> {
    let key = key.into();
    if interaction.draft.is_some() {
        return None;
    }
    if key.modifiers == KeyModifiers::CONTROL {
        let half_page = (height.saturating_sub(2) / 2).max(1);
        match key.code {
            KeyCode::Char('d') => *scroll = scroll.saturating_add(half_page),
            KeyCode::Char('u') => *scroll = scroll.saturating_sub(half_page),
            _ => return None,
        }
        interaction.reveal_selection = false;
        interaction.reveal_pane = false;
        return None;
    }
    if key
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
    {
        return None;
    }
    let items = state.items_for(*view);
    match key.code {
        KeyCode::Char('x') if *view == ui::View::Attention => {
            match interaction
                .selected(items)
                .map(|item| interaction.attention_tracker.capture(item))
            {
                Some(Ok(target)) => interaction.acknowledgement_requested = Some(target),
                Some(Err(error)) => interaction.message = Some(error.to_string()),
                None => {
                    interaction.message =
                        Some("Select a TURN FINISHED item in ATTENTION first.".into())
                }
            }
        }
        KeyCode::Char('f') if *view == ui::View::Sessions => {
            interaction.show_all_panes = !interaction.show_all_panes;
            if let Some(snapshot) = state.snapshot() {
                interaction.sync_panes(snapshot);
            }
            interaction.reveal_pane = true;
            *scroll = 0;
        }
        KeyCode::Down | KeyCode::Char('j') if *view == ui::View::Sessions => {
            if let Some(snapshot) = state.snapshot() {
                interaction.move_pane_selection(snapshot, 1);
            }
        }
        KeyCode::Up | KeyCode::Char('k') if *view == ui::View::Sessions => {
            if let Some(snapshot) = state.snapshot() {
                interaction.move_pane_selection(snapshot, -1);
            }
        }
        KeyCode::Enter | KeyCode::Char('r') if *view == ui::View::Sessions => {
            interaction.registration_requested = state.snapshot().and_then(|snapshot| {
                interaction.sync_panes(snapshot);
                interaction.selected_pane.clone()
            });
            interaction.message = interaction.registration_requested.is_none().then(|| {
                "Select a live pane in SESSIONS first; discovery may be unavailable.".into()
            });
        }
        KeyCode::Down | KeyCode::Char('j') if view.is_item_view() => {
            interaction.move_selection(items, 1)
        }
        KeyCode::Up | KeyCode::Char('k') if view.is_item_view() => {
            interaction.move_selection(items, -1)
        }
        KeyCode::Enter if view.is_item_view() => {
            if let Some(selected) = interaction.selected(items) {
                return Some(selected.item.id.clone());
            }
            interaction.message = Some("Select an item in the current view first.".into());
        }
        KeyCode::Char('r') if view.is_item_view() => interaction.begin_reply(items),
        KeyCode::Char('e' | 'u') if *view == ui::View::Work => {
            interaction.maintenance_requested = interaction.selected(items).map(|selected| {
                if key.code == KeyCode::Char('e') {
                    maintenance::Request::Edit(selected.item.clone())
                } else {
                    maintenance::Request::Unregister(selected.item.clone())
                }
            });
            interaction.message = interaction
                .maintenance_requested
                .is_none()
                .then(|| "Select an item in WORK first.".into());
        }
        KeyCode::Tab if view.is_item_view() => interaction.next_attention(items),
        KeyCode::Char('d') if view.is_item_view() => {
            interaction.review_requested = interaction
                .selected(items)
                .map(|selected| selected.item.id.clone());
            interaction.message = if interaction.review_requested.is_none() {
                Some("Select an item in the current view first.".into())
            } else {
                None
            };
        }
        KeyCode::Down => *scroll = scroll.saturating_add(1),
        KeyCode::Up => *scroll = scroll.saturating_sub(1),
        KeyCode::Home => {
            *scroll = 0;
            if view.is_item_view() {
                interaction.selected_id = None;
                interaction.sync(items);
            } else if let Some(snapshot) = state.snapshot() {
                interaction.selected_pane = None;
                interaction.sync_panes(snapshot);
            }
        }
        KeyCode::Char('a' | 'w' | 's') => {
            *view = match key.code {
                KeyCode::Char('a') => ui::View::Attention,
                KeyCode::Char('w') => ui::View::Work,
                _ => ui::View::Sessions,
            };
            *scroll = 0;
            interaction.sync(state.items_for(*view));
            interaction.reveal_selection = true;
            if *view == ui::View::Sessions {
                if let Some(snapshot) = state.snapshot() {
                    interaction.sync_panes(snapshot);
                }
                interaction.reveal_pane = true;
            }
        }
        _ => {}
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use workbench_core::{AgentStatus, PaneAvailability, WorkItem, WorkItemKind};

    #[test]
    fn acknowledgement_targets_visible_finished_turns_and_keeps_work_and_reply_safety() {
        let mut state = state(&[
            ("hidden", AgentStatus::Running),
            ("finished", AgentStatus::Complete),
            ("waiting", AgentStatus::WaitingForInput),
        ]);
        if let Some(Ok(items)) = &mut state.work_items {
            items[1].completion_fingerprint = Some(99);
        }
        let original = state.items().to_vec();
        let mut interaction = interaction::Interaction::default();
        state.refresh_attention(&mut interaction.attention_tracker);
        let mut view = ui::View::Attention;
        interaction.sync(state.items_for(view));
        navigate(
            KeyCode::Char('x'),
            &state,
            &mut view,
            &mut 0,
            &mut interaction,
            24,
        );
        assert!(interaction.acknowledgement_requested.is_some());
        state.apply_acknowledgement(&mut interaction);
        assert_eq!(state.attention.len(), 1);
        assert_eq!(state.attention[0].item.id, "waiting");
        assert_eq!(interaction.selected_id.as_deref(), Some("waiting"));
        assert_eq!(state.items(), original);
        state.refresh_attention(&mut interaction.attention_tracker);
        assert_eq!(state.attention.len(), 1);
        navigate(
            KeyCode::Char('x'),
            &state,
            &mut view,
            &mut 0,
            &mut interaction,
            24,
        );
        assert!(interaction.acknowledgement_requested.is_none());
        assert!(
            interaction
                .message
                .as_ref()
                .unwrap()
                .contains("input requests cannot be dismissed")
        );
        interaction.begin_reply(&state.attention);
        navigate(
            KeyCode::Char('x'),
            &state,
            &mut view,
            &mut 0,
            &mut interaction,
            24,
        );
        assert!(interaction.acknowledgement_requested.is_none());
        assert_eq!(interaction.draft.as_ref().unwrap().item_id, "waiting");
        interaction.draft = None;
        for mut view in [ui::View::Work, ui::View::Sessions] {
            navigate(
                KeyCode::Char('x'),
                &state,
                &mut view,
                &mut 0,
                &mut interaction,
                24,
            );
            assert!(interaction.acknowledgement_requested.is_none());
        }
        if let Some(Ok(items)) = &mut state.work_items {
            items[1].completion_fingerprint = Some(100);
        }
        state.refresh_attention(&mut interaction.attention_tracker);
        interaction.selected_id = Some("finished".into());
        navigate(
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL),
            &state,
            &mut view,
            &mut 0,
            &mut interaction,
            24,
        );
        assert!(interaction.acknowledgement_requested.is_none());
        navigate(
            KeyCode::Char('x'),
            &state,
            &mut view,
            &mut 0,
            &mut interaction,
            24,
        );
        if let Some(Ok(items)) = &mut state.work_items {
            items[1].status = AgentStatus::WaitingForInput;
            items[1].completion_fingerprint = None;
        }
        state.refresh_attention(&mut interaction.attention_tracker);
        state.apply_acknowledgement(&mut interaction);
        assert_eq!(state.attention.len(), 2);
        assert!(
            interaction
                .message
                .as_ref()
                .unwrap()
                .contains("observed turn changed")
        );
        let empty = AppState::default();
        navigate(
            KeyCode::Char('x'),
            &empty,
            &mut view,
            &mut 0,
            &mut interaction,
            24,
        );
        assert!(interaction.acknowledgement_requested.is_none());
    }

    #[test]
    fn acknowledgement_survives_registry_reconciliation_load_errors_and_focus_refreshes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("items.json");
        let engine = Engine::new(&path);
        let mut state = state(&[("A", AgentStatus::Complete)]);
        if let Some(Ok(items)) = &mut state.work_items {
            items[0].completion_fingerprint = Some(42);
        }
        let original = state.items()[0].item.clone();
        engine.register_work_item(original.clone()).unwrap();
        let mut snapshot = Snapshot::default();
        snapshot.sessions.push(workbench_core::Session {
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
        });
        state.discovery = Some(Ok(snapshot));
        let mut interaction = interaction::Interaction::default();
        state.refresh_attention(&mut interaction.attention_tracker);
        let target = interaction
            .attention_tracker
            .capture(&state.attention[0])
            .unwrap();
        interaction.attention_tracker.acknowledge(&target).unwrap();
        engine
            .update_work_item_description(&original, "New description")
            .unwrap();
        state.reload_registry(&engine);
        state.refresh_attention(&mut interaction.attention_tracker);
        assert!(state.attention.is_empty());
        assert_eq!(state.items()[0].item.title, "New description");
        let resumed_items = state.items().to_vec();
        state.work_items = Some(Err("state busy".into()));
        state.refresh_attention(&mut interaction.attention_tracker);
        state.work_items = Some(Ok(resumed_items.clone()));
        state.refresh_attention(&mut interaction.attention_tracker);
        assert!(state.attention.is_empty());
        // A fresh watcher after pane focus reuses the session-owned tracker.
        let mut resumed =
            AppState::from_refresh(state.discovery.clone().unwrap(), Ok(resumed_items));
        resumed.refresh_attention(&mut interaction.attention_tracker);
        assert!(resumed.attention.is_empty());
        resumed.refresh_attention(&mut workbench_core::AttentionTracker::default());
        assert_eq!(resumed.attention.len(), 1);
    }

    #[test]
    fn local_rename_keeps_selection_observed_status_and_acknowledgement() {
        let directory = tempfile::tempdir().unwrap();
        let engine = Engine::new(directory.path().join("items.json"));
        let mut state = state(&[("A", AgentStatus::Complete)]);
        if let Some(Ok(items)) = &mut state.work_items {
            items[0].completion_fingerprint = Some(42);
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
        state.refresh_attention(&mut interaction.attention_tracker);
        let captured = interaction
            .attention_tracker
            .capture(&state.items()[0])
            .unwrap();
        interaction
            .attention_tracker
            .acknowledge(&captured)
            .unwrap();
        let mutation = maintenance::Mutation::Details {
            expected: original.clone(),
            id: "New λ🙂".into(),
            description: "Updated description".into(),
        };
        mutation.execute(&engine).unwrap();
        state.apply_detail_edit(&mutation, &mut interaction);
        state.reload_registry(&engine);
        state.refresh_attention(&mut interaction.attention_tracker);
        interaction.sync(state.items());
        assert_eq!(interaction.selected_id.as_deref(), Some("New λ🙂"));
        assert_eq!(state.items()[0].status, AgentStatus::Complete);
        assert_eq!(state.items()[0].completion_fingerprint, Some(42));
        assert_eq!(state.items()[0].item.title, "Updated description");
        assert!(state.attention.is_empty());
        assert!(
            interaction
                .attention_tracker
                .acknowledge(&captured)
                .is_err()
        );
        let mut resumed =
            AppState::from_refresh(state.discovery.clone().unwrap(), Ok(state.items().to_vec()));
        resumed.refresh_attention(&mut interaction.attention_tracker);
        assert!(resumed.attention.is_empty());
    }

    #[test]
    fn work_maintenance_actions_capture_selection_and_never_target_hidden_items() {
        let state = state(&[
            ("hidden", AgentStatus::Running),
            ("A", AgentStatus::Complete),
        ]);
        for mut view in [ui::View::Work, ui::View::Attention, ui::View::Sessions] {
            let mut interaction = interaction::Interaction::default();
            interaction.sync(state.items_for(view));
            for code in [KeyCode::Char('e'), KeyCode::Char('n'), KeyCode::Char('u')] {
                navigate(code, &state, &mut view, &mut 0, &mut interaction, 24);
                if view == ui::View::Work && code != KeyCode::Char('n') {
                    match interaction.maintenance_requested.take().unwrap() {
                        maintenance::Request::Edit(item) => {
                            assert_eq!(code, KeyCode::Char('e'));
                            assert_eq!(item.id, "hidden");
                        }
                        maintenance::Request::Unregister(item) => {
                            assert_eq!(code, KeyCode::Char('u'));
                            assert_eq!(item.id, "hidden");
                        }
                    }
                } else {
                    assert!(interaction.maintenance_requested.is_none());
                }
            }
        }
        let mut view = ui::View::Work;
        let mut interaction = interaction::Interaction::default();
        navigate(
            KeyCode::Char('e'),
            &AppState::default(),
            &mut view,
            &mut 0,
            &mut interaction,
            24,
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
                &mut view,
                &mut 0,
                &mut interaction,
                24,
            );
            assert!(interaction.maintenance_requested.is_none());
        }
        interaction.begin_reply(state.items());
        navigate(
            KeyCode::Char('u'),
            &state,
            &mut view,
            &mut 0,
            &mut interaction,
            24,
        );
        assert!(interaction.maintenance_requested.is_none());
    }

    #[test]
    fn registry_reconciliation_removes_stale_refresh_items_and_keeps_live_observations() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("items.json");
        let engine = Engine::new(&path);
        let mut stale = state(&[("A", AgentStatus::Running), ("B", AgentStatus::Complete)]);
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
        engine.unregister_work_item(&b).unwrap();
        let mut pending_refresh = stale;
        pending_refresh.reload_registry(&engine);
        assert_eq!(pending_refresh.items().len(), 1);
        assert_eq!(pending_refresh.items()[0].item.title, "New description");
        assert!(pending_refresh.attention.is_empty());
        engine.unregister_work_item(&updated).unwrap();
        let mut replacement = updated;
        replacement.pane_id = "%99".into();
        engine.register_work_item(replacement).unwrap();
        pending_refresh.reload_registry(&engine);
        assert_eq!(pending_refresh.items()[0].status, AgentStatus::Unknown);
        std::fs::write(&path, "{broken").unwrap();
        pending_refresh.reload_registry(&engine);
        assert!(matches!(pending_refresh.work_items, Some(Err(_))));
        assert!(pending_refresh.attention.is_empty());
    }

    #[test]
    fn sessions_filter_toggle_selection_and_registration_never_target_hidden_panes() {
        use workbench_core::{Pane, Session, Window};
        let mut state = state(&[("registered-shell", AgentStatus::Unknown)]);
        state.discovery = Some(Ok(Snapshot {
            sessions: vec![Session {
                id: "$1".into(),
                name: "main".into(),
                windows: vec![Window {
                    id: "@1".into(),
                    index: 0,
                    name: "mixed".into(),
                    panes: [("%1", "fish"), ("%2", "claude"), ("%3", "codex")]
                        .into_iter()
                        .map(|(id, command)| Pane {
                            id: id.into(),
                            index: 0,
                            title: "Agent".into(),
                            current_command: Some(command.into()),
                            working_directory: None,
                        })
                        .collect(),
                }],
            }],
        }));
        let mut interaction = interaction::Interaction {
            selected_pane: Some("%1".into()),
            ..Default::default()
        };
        let mut view = ui::View::Sessions;
        let mut scroll = 25;
        navigate(
            KeyCode::Enter,
            &state,
            &mut view,
            &mut scroll,
            &mut interaction,
            24,
        );
        assert_eq!(
            interaction.registration_requested.take().as_deref(),
            Some("%2")
        );
        navigate(
            KeyCode::Char('j'),
            &state,
            &mut view,
            &mut scroll,
            &mut interaction,
            24,
        );
        assert_eq!(interaction.selected_pane.as_deref(), Some("%3"));
        navigate(
            KeyCode::Char('f'),
            &state,
            &mut view,
            &mut scroll,
            &mut interaction,
            24,
        );
        assert!(interaction.show_all_panes);
        assert_eq!(interaction.selected_pane.as_deref(), Some("%3"));
        assert_eq!(scroll, 0);
        navigate(
            KeyCode::Home,
            &state,
            &mut view,
            &mut scroll,
            &mut interaction,
            24,
        );
        navigate(
            KeyCode::Char('r'),
            &state,
            &mut view,
            &mut scroll,
            &mut interaction,
            24,
        );
        assert_eq!(
            interaction.registration_requested.take().as_deref(),
            Some("%1")
        );
        navigate(
            KeyCode::Char('f'),
            &state,
            &mut view,
            &mut scroll,
            &mut interaction,
            24,
        );
        assert_eq!(interaction.selected_pane.as_deref(), Some("%2"));
        assert_eq!(state.items().len(), 1);
        assert_eq!(
            state.snapshot().unwrap().sessions[0].windows[0].panes.len(),
            3
        );
        if let Some(Ok(snapshot)) = &mut state.discovery {
            for pane in &mut snapshot.sessions[0].windows[0].panes {
                pane.current_command = Some("fish".into());
            }
        }
        navigate(
            KeyCode::Enter,
            &state,
            &mut view,
            &mut scroll,
            &mut interaction,
            24,
        );
        assert!(interaction.selected_pane.is_none());
        assert!(interaction.registration_requested.is_none());
        assert!(
            interaction
                .message
                .as_ref()
                .unwrap()
                .contains("Select a live pane")
        );
        navigate(
            KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL),
            &state,
            &mut view,
            &mut scroll,
            &mut interaction,
            24,
        );
        assert!(!interaction.show_all_panes);
        view = ui::View::Work;
        navigate(
            KeyCode::Char('f'),
            &state,
            &mut view,
            &mut scroll,
            &mut interaction,
            24,
        );
        assert!(!interaction.show_all_panes);
    }

    #[test]
    fn sessions_navigation_requests_registration_only_for_a_live_selected_pane() {
        use workbench_core::{Pane, Session, Window};
        let mut state = state(&[]);
        state.discovery = Some(Ok(Snapshot {
            sessions: vec![Session {
                id: "$1".into(),
                name: "main".into(),
                windows: vec![Window {
                    id: "@1".into(),
                    index: 0,
                    name: "task".into(),
                    panes: ["%1", "%2"]
                        .into_iter()
                        .map(|id| Pane {
                            id: id.into(),
                            index: 0,
                            title: "Agent".into(),
                            current_command: Some("codex".into()),
                            working_directory: None,
                        })
                        .collect(),
                }],
            }],
        }));
        let mut view = ui::View::Sessions;
        let mut scroll = 0;
        let mut interaction = interaction::Interaction::default();
        navigate(
            KeyCode::Char('j'),
            &state,
            &mut view,
            &mut scroll,
            &mut interaction,
            24,
        );
        assert_eq!(interaction.selected_pane.as_deref(), Some("%2"));
        navigate(
            KeyCode::Enter,
            &state,
            &mut view,
            &mut scroll,
            &mut interaction,
            24,
        );
        assert_eq!(
            interaction.registration_requested.take().as_deref(),
            Some("%2")
        );
        navigate(
            KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL),
            &state,
            &mut view,
            &mut scroll,
            &mut interaction,
            24,
        );
        assert!(interaction.registration_requested.is_none());
        assert!(interaction.draft.is_none());
        state.discovery = Some(Err("Server unavailable".into()));
        navigate(
            KeyCode::Char('r'),
            &state,
            &mut view,
            &mut scroll,
            &mut interaction,
            24,
        );
        assert!(interaction.registration_requested.is_none());
        assert!(interaction.message.as_ref().unwrap().contains("live pane"));
        state.discovery = Some(Ok(Snapshot::default()));
        navigate(
            KeyCode::Enter,
            &state,
            &mut view,
            &mut scroll,
            &mut interaction,
            24,
        );
        assert!(interaction.registration_requested.is_none());
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
                    status_detail: "Observed locally.".into(),
                    attention_prompt: None,
                    completion_fingerprint: None,
                })
                .collect()),
        )
    }

    #[test]
    fn vim_half_page_scroll_works_in_every_view_without_changing_selection() {
        let state = state(&[("A", AgentStatus::WaitingForInput)]);
        for mut view in [ui::View::Attention, ui::View::Work, ui::View::Sessions] {
            let original_view = view;
            let mut interaction = interaction::Interaction::default();
            interaction.sync(state.items_for(view));
            let selected = interaction.selected_id.clone();
            let mut scroll = 0;
            for (key, expected) in [('d', 11), ('d', 22), ('u', 11), ('u', 0), ('u', 0)] {
                assert!(
                    navigate(
                        KeyEvent::new(KeyCode::Char(key), KeyModifiers::CONTROL),
                        &state,
                        &mut view,
                        &mut scroll,
                        &mut interaction,
                        24,
                    )
                    .is_none()
                );
                assert_eq!(scroll, expected);
                assert_eq!(view, original_view);
                assert_eq!(interaction.selected_id, selected);
                assert!(interaction.message.is_none());
                assert!(interaction.draft.is_none());
                assert!(!interaction.reveal_selection);
            }
            scroll = 7;
            navigate(
                KeyCode::PageDown,
                &state,
                &mut view,
                &mut scroll,
                &mut interaction,
                24,
            );
            assert_eq!(scroll, 7);
            navigate(
                KeyCode::PageUp,
                &state,
                &mut view,
                &mut scroll,
                &mut interaction,
                24,
            );
            assert_eq!(scroll, 7);
        }
    }

    #[test]
    fn vim_scroll_saturates_and_moves_at_least_one_row_on_tiny_terminals() {
        let state = state(&[]);
        let mut view = ui::View::Attention;
        let mut interaction = interaction::Interaction::default();
        for height in [0, 1, 2, 3, 4] {
            let mut scroll = 0;
            navigate(
                KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
                &state,
                &mut view,
                &mut scroll,
                &mut interaction,
                height,
            );
            assert_eq!(scroll, 1);
            scroll = u16::MAX;
            navigate(
                KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
                &state,
                &mut view,
                &mut scroll,
                &mut interaction,
                height,
            );
            assert_eq!(scroll, u16::MAX);
        }
    }

    #[test]
    fn modified_keys_and_reply_drafts_do_not_trigger_list_actions() {
        let state = state(&[("A", AgentStatus::WaitingForInput)]);
        let mut view = ui::View::Attention;
        let mut interaction = interaction::Interaction::default();
        interaction.sync(state.items_for(view));
        let mut scroll = 10;
        for key in [
            KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL),
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::ALT),
            KeyEvent::new(
                KeyCode::Char('d'),
                KeyModifiers::CONTROL | KeyModifiers::ALT,
            ),
        ] {
            navigate(key, &state, &mut view, &mut scroll, &mut interaction, 24);
            assert_eq!(scroll, 10);
            assert!(interaction.message.is_none());
            assert!(interaction.draft.is_none());
        }
        interaction.begin_reply(state.items_for(view));
        interaction.draft.as_mut().unwrap().append("yes").unwrap();
        for ch in ['d', 'u'] {
            navigate(
                KeyEvent::new(KeyCode::Char(ch), KeyModifiers::CONTROL),
                &state,
                &mut view,
                &mut scroll,
                &mut interaction,
                24,
            );
            assert_eq!(scroll, 10);
            assert_eq!(interaction.draft.as_ref().unwrap().text, "yes");
        }
    }

    #[test]
    fn attention_is_default_and_all_actions_target_the_visible_queue() {
        let state = state(&[
            ("hidden", AgentStatus::Running),
            ("B", AgentStatus::Complete),
            ("C", AgentStatus::WaitingForInput),
        ]);
        let mut view = ui::View::default();
        assert_eq!(view, ui::View::Attention);
        let mut interaction = interaction::Interaction::default();
        interaction.sync(state.items_for(view));
        let mut scroll = 0;
        assert_eq!(interaction.selected_id.as_deref(), Some("B"));
        navigate(
            KeyCode::Char('j'),
            &state,
            &mut view,
            &mut scroll,
            &mut interaction,
            24,
        );
        assert_eq!(
            navigate(
                KeyCode::Enter,
                &state,
                &mut view,
                &mut scroll,
                &mut interaction,
                24
            )
            .as_deref(),
            Some("C")
        );
        navigate(
            KeyCode::Char('r'),
            &state,
            &mut view,
            &mut scroll,
            &mut interaction,
            24,
        );
        assert_eq!(interaction.draft.as_ref().unwrap().item_id, "C");
        interaction.draft = None;
        navigate(
            KeyCode::Tab,
            &state,
            &mut view,
            &mut scroll,
            &mut interaction,
            24,
        );
        assert_eq!(interaction.selected_id.as_deref(), Some("B"));
        navigate(
            KeyCode::Char('d'),
            &state,
            &mut view,
            &mut scroll,
            &mut interaction,
            24,
        );
        assert_eq!(interaction.review_requested.as_deref(), Some("B"));
        navigate(
            KeyCode::Char('w'),
            &state,
            &mut view,
            &mut scroll,
            &mut interaction,
            24,
        );
        assert_eq!(view, ui::View::Work);
        navigate(
            KeyCode::Home,
            &state,
            &mut view,
            &mut scroll,
            &mut interaction,
            24,
        );
        assert_eq!(interaction.selected_id.as_deref(), Some("hidden"));
        navigate(
            KeyCode::Char('a'),
            &state,
            &mut view,
            &mut scroll,
            &mut interaction,
            24,
        );
        assert_eq!(interaction.selected_id.as_deref(), Some("B"));
        navigate(
            KeyCode::Char('s'),
            &state,
            &mut view,
            &mut scroll,
            &mut interaction,
            24,
        );
        assert_eq!(view, ui::View::Sessions);
        assert!(
            navigate(
                KeyCode::Enter,
                &state,
                &mut view,
                &mut scroll,
                &mut interaction,
                24
            )
            .is_none()
        );
    }

    #[test]
    fn refresh_changes_queue_without_redirecting_an_existing_draft() {
        let initial = state(&[
            ("A", AgentStatus::WaitingForInput),
            ("B", AgentStatus::Complete),
        ]);
        let mut interaction = interaction::Interaction::default();
        interaction.sync(initial.items_for(ui::View::Attention));
        interaction.begin_reply(initial.items_for(ui::View::Attention));
        let next = state(&[("A", AgentStatus::Running), ("B", AgentStatus::Complete)]);
        interaction.sync(next.items_for(ui::View::Attention));
        assert_eq!(interaction.selected_id.as_deref(), Some("B"));
        assert_eq!(interaction.draft.as_ref().unwrap().item_id, "A");
        let empty = state(&[("A", AgentStatus::Running), ("B", AgentStatus::Unknown)]);
        interaction.sync(empty.items_for(ui::View::Attention));
        assert!(interaction.selected_id.is_none());
        assert_eq!(interaction.draft.as_ref().unwrap().item_id, "A");
        assert_eq!(empty.items().len(), 2);
    }

    #[test]
    fn empty_or_failed_queue_never_opens_or_replies_to_hidden_work() {
        let empty = state(&[("hidden", AgentStatus::Unknown)]);
        let failed = AppState::from_refresh(
            Err("tmux unavailable".into()),
            Err("state unreadable".into()),
        );
        for state in [empty, failed] {
            let mut view = ui::View::Attention;
            let mut scroll = 0;
            let mut interaction = interaction::Interaction {
                selected_id: Some("hidden".into()),
                ..Default::default()
            };
            assert!(
                navigate(
                    KeyCode::Enter,
                    &state,
                    &mut view,
                    &mut scroll,
                    &mut interaction,
                    24
                )
                .is_none()
            );
            navigate(
                KeyCode::Char('r'),
                &state,
                &mut view,
                &mut scroll,
                &mut interaction,
                24,
            );
            assert!(interaction.draft.is_none());
            assert!(state.attention.is_empty());
            navigate(
                KeyCode::Char('d'),
                &state,
                &mut view,
                &mut scroll,
                &mut interaction,
                24,
            );
            assert!(interaction.review_requested.is_none());
        }
    }
}
