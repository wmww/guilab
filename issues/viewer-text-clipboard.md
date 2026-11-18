# Copy/paste doesn't work in the viewer's own text fields

Chat composer, size/zoom fields, editor text: Ctrl+C/X/V do nothing.

Why: egui text fields act only on `Event::Copy`/`Cut`/`Paste(text)` input and report copies as `OutputCommand::CopyText` in `platform_output.commands`. The viewer (`crates/viewer/src/host.rs` key handling, `lib.rs` after `run_ui`) produces neither event and ignores the commands; Ctrl+C reaches egui as a plain key.

Fix sketch: map Ctrl+C/X/V (and Shift+Insert etc.) to those events while egui has focus; paste needs the host selection's text (`HostClipboard` already reads selections, but only on change and on a thread, so keep the latest host text or request it async); send `CopyText` to `HostClipboard::set_clipboard`. Keep the session bridge (`ClipBridge`) separate: copying in our own fields mustn't go to the session.
