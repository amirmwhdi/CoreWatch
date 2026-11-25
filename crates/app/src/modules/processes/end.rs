//! End Process and Force Stop: the confirmation dialog, then the signal
//! sent off the main thread.

use super::row;
use crate::format;
use adw::prelude::*;
use corewatch_core::process::{send_signal, ProcessSignal, ProcessSource};
use corewatch_core::SysRoot;
use gtk::{gio, glib};
use std::io;

/// Ask before ending a process, then send the signal off the main thread.
pub(crate) fn confirm_end(
    parent: &gtk::Widget,
    toasts: &adw::ToastOverlay,
    process: &row::ProcessRow,
) {
    // A group ends all its processes.
    let targets: Vec<(u32, u64)> = match process.children() {
        Some(children) => children
            .iter::<row::ProcessRow>()
            .flatten()
            .map(|r| r.key())
            .collect(),
        None => vec![process.key()],
    };
    let heading = if process.is_group() {
        format::fmt(
            format::plural(
                targets.len(),
                "End the process of “{name}”?",
                "End all {n} processes of “{name}”?",
            ),
            &[("name", &process.group_name()), ("n", &targets.len())],
        )
    } else {
        format::fmt("End “{name}”?", &[("name", &process.name())])
    };
    let dialog = adw::AlertDialog::new(
        Some(&heading),
        Some("Unsaved work in this program may be lost. Force Stop ends it at once, without letting it clean up."),
    );
    dialog.add_responses(&[
        ("cancel", "_Cancel"),
        ("kill", "_Force Stop"),
        ("end", "_End Process"),
    ]);
    dialog.set_response_appearance("kill", adw::ResponseAppearance::Destructive);
    dialog.set_response_appearance("end", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("end"));
    dialog.set_close_response("cancel");

    let toasts = toasts.clone();
    let source = ProcessSource::detect(&SysRoot::from_env());
    dialog.choose(Some(parent), gio::Cancellable::NONE, move |response| {
        let signal = match response.as_str() {
            "end" => ProcessSignal::End,
            "kill" => ProcessSignal::ForceStop,
            _ => return,
        };
        glib::spawn_future_local(async move {
            let count = targets.len();
            let results = gio::spawn_blocking(move || {
                targets
                    .into_iter()
                    .map(|(pid, start_ticks)| {
                        let result = send_signal(source, pid, start_ticks, signal);
                        if let Err(error) = &result {
                            if error.kind() != io::ErrorKind::NotFound {
                                tracing::warn!(pid, %error, "cannot end process");
                            }
                        }
                        result
                    })
                    .collect::<Vec<_>>()
            })
            .await
            .unwrap_or_default();
            // Gone already counts as ended.
            let failed: Vec<&io::Error> = results
                .iter()
                .filter_map(|r| r.as_ref().err())
                .filter(|e| e.kind() != io::ErrorKind::NotFound)
                .collect();
            let message = if results.is_empty() {
                String::from("Could not end the process")
            } else if failed.is_empty() {
                if count == 1 && results.iter().all(Result::is_err) {
                    String::from("The process has already ended")
                } else {
                    return;
                }
            } else if failed
                .iter()
                .all(|e| e.kind() == io::ErrorKind::PermissionDenied)
            {
                String::from("You can only end your own processes")
            } else if count == 1 {
                String::from("Could not end the process")
            } else {
                format::fmt(
                    "Could not end {failed} of {count} processes",
                    &[("failed", &failed.len()), ("count", &count)],
                )
            };
            toasts.add_toast(adw::Toast::new(&message));
        });
    });
}
