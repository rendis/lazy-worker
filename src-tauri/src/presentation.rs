use crate::{activity, panel, tray, Controller};
use std::{
    io::{self, BufRead, BufReader},
    process::{ChildStdin, Stdio},
    sync::{Arc, Mutex},
    thread,
};
use tauri::{AppHandle, Manager};

#[derive(Default)]
pub struct Presentation {
    watch: Option<Watch>,
    hidden: bool,
    restore_panel: Option<bool>,
}

struct Watch {
    // Closing the pipe tells the shared Activity helper to exit.
    _input: ChildStdin,
    session: Arc<()>,
}

pub fn refresh(app: &AppHandle) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || update(&handle));
}

fn update(app: &AppHandle) {
    let controller = app.state::<Controller>();
    let Ok(tools) = controller.try_lock() else {
        return;
    };
    let enabled = tools.activity.preferences().hide_while_sharing;
    drop(tools);

    let state = app.state::<Mutex<Presentation>>();
    let Ok(mut presentation) = state.lock() else {
        return;
    };
    if !enabled {
        presentation.watch.take();
        presentation.hidden = false;
    } else if presentation.watch.is_none() {
        match start(app) {
            Ok(watch) => presentation.watch = Some(watch),
            Err(error) => eprintln!("Could not start the presentation monitor: {error}"),
        }
    }
    presentation.update_visibility(app);
}

pub fn is_hidden(app: &AppHandle) -> bool {
    app.state::<Mutex<Presentation>>()
        .try_lock()
        .map_or(true, |state| state.hidden || state.restore_panel.is_some())
}

pub fn stop(app: &AppHandle) {
    if let Ok(mut state) = app.state::<Mutex<Presentation>>().lock() {
        state.watch.take();
    }
}

fn start(app: &AppHandle) -> io::Result<Watch> {
    let mut child = activity::helper_command()?
        .arg("--monitor-presentation")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let input = child.stdin.take().expect("Presentation stdin is piped");
    let output = child.stdout.take().expect("Presentation stdout is piped");
    let session = Arc::new(());
    let reader_session = session.clone();
    let handle = app.clone();
    thread::spawn(move || {
        for line in BufReader::new(output).lines().map_while(Result::ok) {
            match line.as_str() {
                "hidden" => dispatch(&handle, &reader_session, Some(true)),
                "visible" => dispatch(&handle, &reader_session, Some(false)),
                _ => {}
            }
        }
        dispatch(&handle, &reader_session, None);
        let _ = child.wait();
    });
    Ok(Watch {
        _input: input,
        session,
    })
}

fn dispatch(app: &AppHandle, session: &Arc<()>, hidden: Option<bool>) {
    let handle = app.clone();
    let session = session.clone();
    let _ = app.run_on_main_thread(move || {
        let state = handle.state::<Mutex<Presentation>>();
        let Ok(mut presentation) = state.lock() else {
            return;
        };
        if !presentation
            .watch
            .as_ref()
            .is_some_and(|watch| Arc::ptr_eq(&watch.session, &session))
        {
            return;
        }
        if hidden.is_none() {
            presentation.watch.take();
        }
        presentation.hidden = hidden.unwrap_or(false);
        presentation.update_visibility(&handle);
    });
}

impl Presentation {
    fn update_visibility(&mut self, app: &AppHandle) {
        if let Err(error) = self.apply_visibility(app) {
            eprintln!("Could not change presentation visibility: {error}");
        }
    }

    fn apply_visibility(&mut self, app: &AppHandle) -> tauri::Result<()> {
        match (self.hidden, self.restore_panel) {
            (true, None) => {
                tray::set_visible(app, false)?;
                match panel::hide_for_presentation(app) {
                    Ok(was_visible) => self.restore_panel = Some(was_visible),
                    Err(error) => {
                        let _ = tray::set_visible(app, true);
                        return Err(error);
                    }
                }
            }
            (false, Some(was_visible)) => {
                tray::set_visible(app, true)?;
                if was_visible {
                    panel::restore_after_presentation(app)?;
                }
                self.restore_panel = None;
            }
            _ => {}
        }
        Ok(())
    }
}
