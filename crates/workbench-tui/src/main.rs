use std::{io, time::Duration};

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use tokio::{
    sync::watch,
    time::{self, MissedTickBehavior},
};
use workbench_core::{Engine, Snapshot};

mod ui;

type DiscoveryState = Option<Result<Snapshot, String>>;

#[tokio::main(flavor = "current_thread")]
async fn main() -> io::Result<()> {
    let mut terminal = match ratatui::try_init() {
        Ok(terminal) => terminal,
        Err(error) => {
            ratatui::restore();
            return Err(error);
        }
    };
    let result = run(&mut terminal).await;
    let cleanup = ratatui::try_restore();
    result.and(cleanup)
}

async fn run(terminal: &mut ratatui::DefaultTerminal) -> io::Result<()> {
    let (sender, mut receiver) = watch::channel::<DiscoveryState>(None);
    let discovery = tokio::spawn(async move {
        let engine = Engine;
        let mut refresh = time::interval(Duration::from_secs(2));
        refresh.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            refresh.tick().await;
            let result = engine.discover().await.map_err(|error| error.to_string());
            if sender.send(Some(result)).is_err() {
                break;
            }
        }
    });
    let result = event_loop(terminal, &mut receiver).await;
    discovery.abort();
    // Wait for cancellation so an in-flight command is dropped and killed.
    let _ = discovery.await;
    result
}

async fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    receiver: &mut watch::Receiver<DiscoveryState>,
) -> io::Result<()> {
    let mut state = None;
    let mut scroll = 0_u16;
    let mut redraw = true;
    let mut input_tick = time::interval(Duration::from_millis(100));
    input_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        if redraw {
            terminal.draw(|frame| ui::render(frame, &state, &mut scroll))?;
            redraw = false;
        }
        tokio::select! {
            changed = receiver.changed() => {
                changed.map_err(|_| io::Error::other("Discovery task stopped unexpectedly"))?;
                state = receiver.borrow_and_update().clone();
                redraw = true;
            }
            _ = input_tick.tick() => {
                while event::poll(Duration::ZERO)? {
                    redraw = true;
                    if let Event::Key(key) = event::read()? {
                        if key.kind == KeyEventKind::Release {
                            continue;
                        }
                        match key.code {
                            KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return Ok(()),
                            KeyCode::Down => scroll = scroll.saturating_add(1),
                            KeyCode::Up => scroll = scroll.saturating_sub(1),
                            KeyCode::PageDown => scroll = scroll.saturating_add(terminal.size()?.height.saturating_sub(3)),
                            KeyCode::PageUp => scroll = scroll.saturating_sub(terminal.size()?.height.saturating_sub(3)),
                            KeyCode::Home => scroll = 0,
                            _ => {}
                        }
                    }
                }
            }
        }
    }
}
