//! Translation plumbing.
//!
//! Every string a user sees goes through [`gettext`] or [`ngettext`]. No
//! translation ships with Corewatch yet; with no `.mo` file installed these
//! calls return the English text unchanged. Log messages and `--help` stay in
//! English on purpose, so bug reports are readable by everyone.

use std::fmt::Display;

pub use gettextrs::{gettext, ngettext};

/// Text domain: the `.mo` files are named `corewatch.mo`.
pub const GETTEXT_PACKAGE: &str = "corewatch";

/// Where installed translations live. Meson sets `COREWATCH_LOCALEDIR` at
/// build time; a plain Cargo build uses the usual system path.
pub const LOCALEDIR: &str = match option_env!("COREWATCH_LOCALEDIR") {
    Some(dir) => dir,
    None => "/usr/share/locale",
};

/// Bind the text domain. Call once, before the first window is built.
///
/// The locale itself is not set here: GTK calls `setlocale(LC_ALL, "")` while
/// it starts up, before `activate` builds any window, and doing it ourselves
/// would need `unsafe` (setlocale is not thread-safe and the log writer thread
/// is already running).
pub fn init() {
    use gettextrs::{bind_textdomain_codeset, bindtextdomain, textdomain};

    if let Err(error) = bindtextdomain(GETTEXT_PACKAGE, LOCALEDIR) {
        tracing::warn!(%error, localedir = LOCALEDIR, "cannot bind text domain");
    }
    if let Err(error) = bind_textdomain_codeset(GETTEXT_PACKAGE, "UTF-8") {
        tracing::warn!(%error, "cannot set text domain codeset");
    }
    if let Err(error) = textdomain(GETTEXT_PACKAGE) {
        tracing::warn!(%error, "cannot select text domain");
    }
}

/// Fill named placeholders in a translated template:
/// `fmt(&gettext("{used} of {total}"), &[("used", &a), ("total", &b)])`.
///
/// Named placeholders let a translation reorder the values, which positional
/// `format!` arguments cannot. Unknown or unclosed placeholders are left as
/// they are, and a value is never scanned for placeholders itself.
pub fn fmt(template: &str, args: &[(&str, &dyn Display)]) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let value = after.find('}').and_then(|end| {
            args.iter()
                .find(|(name, _)| *name == &after[..end])
                .map(|(_, v)| (end, v))
        });
        match value {
            Some((end, value)) => {
                out.push_str(&value.to_string());
                rest = &after[end + 1..];
            }
            None => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// A share 0.0..=1.0 as a whole percentage, e.g. "37%". The template is
/// translatable because some languages put the sign first.
pub fn percent(fraction: f32) -> String {
    let value = format!("{:.0}", fraction * 100.0);
    // Translators: a percentage, e.g. "37%"
    fmt(&gettext("{value}%"), &[("value", &value)])
}

/// `ngettext` needs a `u32`; counts above that are not a real concern here.
pub fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::fmt;

    #[test]
    fn fills_named_placeholders_in_any_order() {
        let text = fmt(
            "{total} total, {used} used",
            &[("used", &1), ("total", &"2 GB")],
        );
        assert_eq!(text, "2 GB total, 1 used");
    }

    #[test]
    fn repeats_and_unknown_names_are_handled() {
        assert_eq!(fmt("{a}-{a}", &[("a", &7)]), "7-7");
        assert_eq!(fmt("{b} {a}", &[("a", &1)]), "{b} 1");
        assert_eq!(fmt("open { brace", &[]), "open { brace");
    }

    #[test]
    fn values_are_not_expanded_again() {
        assert_eq!(fmt("{a}", &[("a", &"{a}")]), "{a}");
    }

    #[test]
    fn non_ascii_text_around_placeholders() {
        assert_eq!(fmt("— {n} —", &[("n", &3)]), "— 3 —");
    }
}
