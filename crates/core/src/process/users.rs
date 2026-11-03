//! User names by uid.
//!
//! `/etc/passwd` is read once. A uid missing from it (systemd-homed, LDAP,
//! …) is looked up in the system's user databases on a separate thread:
//! such a lookup may go over the network, and the process scan must not
//! wait for it. Until the answer comes, the uid is shown as a number.

use super::parse::parse_passwd;
use super::ProcessSource;
use crate::SysRoot;
use std::collections::{HashMap, HashSet};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

/// How long a scan waits for a new uid's name. A local database answers in
/// microseconds; a slow one is shown as the uid until it answers.
pub const LOOKUP_WAIT: Duration = Duration::from_millis(50);

/// Uids to the lookup thread, and its answers back.
type Lookups = (mpsc::Sender<u32>, mpsc::Receiver<(u32, Option<String>)>);

pub(super) struct UserNames {
    /// Names known so far, and uids no database knows (as their number).
    names: HashMap<u32, Arc<str>>,
    /// Uids sent to the lookup thread.
    asked: HashSet<u32>,
    lookups: Option<Lookups>,
}

impl UserNames {
    /// `passwd`: the text of `/etc/passwd`. Other uids are looked up only for
    /// the real machine, not a test root.
    pub fn new(passwd: &str, root: &SysRoot, source: ProcessSource) -> Self {
        let names = parse_passwd(passwd)
            .into_iter()
            .map(|(uid, name)| (uid, Arc::from(name)))
            .collect();
        Self {
            names,
            asked: HashSet::new(),
            lookups: root.is_host().then(|| spawn(source)).flatten(),
        }
    }

    /// The name of `uid`, or the uid as text while it is not known (yet).
    pub fn get(&mut self, uid: u32) -> Arc<str> {
        if let Some(name) = self.names.get(&uid) {
            return Arc::clone(name);
        }
        if let Some((requests, _)) = &self.lookups {
            if self.asked.insert(uid) && requests.send(uid).is_ok() {
                self.receive(LOOKUP_WAIT);
            } else {
                self.receive(Duration::ZERO);
            }
        }
        self.names
            .get(&uid)
            .cloned()
            .unwrap_or_else(|| Arc::from(uid.to_string()))
    }

    /// Take the answers that are in, waiting up to `wait` for the first.
    fn receive(&mut self, wait: Duration) {
        let Some((_, answers)) = &self.lookups else {
            return;
        };
        let first = if wait.is_zero() {
            answers.try_recv().ok()
        } else {
            answers.recv_timeout(wait).ok()
        };
        for (uid, name) in first.into_iter().chain(answers.try_iter()) {
            let name = name.unwrap_or_else(|| uid.to_string());
            self.names.insert(uid, Arc::from(name));
        }
    }
}

/// The lookup thread; it ends when [`UserNames`] is dropped.
fn spawn(source: ProcessSource) -> Option<Lookups> {
    let (requests, todo) = mpsc::channel::<u32>();
    let (done, answers) = mpsc::channel();
    std::thread::Builder::new()
        .name("corewatch-users".into())
        .spawn(move || {
            for uid in todo {
                let name = match source {
                    ProcessSource::Direct => name_from_nss(uid),
                    ProcessSource::FlatpakHost => name_from_host(uid),
                };
                if done.send((uid, name)).is_err() {
                    break;
                }
            }
        })
        .map_err(|e| tracing::warn!("cannot start the user name thread: {e}"))
        .ok()?;
    Some((requests, answers))
}

/// `getpwuid_r`: asks every user database configured in nsswitch.conf.
pub(super) fn name_from_nss(uid: u32) -> Option<String> {
    let mut buf = vec![0u8; 1024];
    loop {
        // SAFETY: an all-zero `passwd` is a valid value (null pointers, zero
        // ids); getpwuid_r only writes into `pwd` and `buf`, both alive and
        // `buf.len()` long, and sets `found` to `&pwd` or null.
        let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
        let mut found: *mut libc::passwd = std::ptr::null_mut();
        let err = unsafe {
            libc::getpwuid_r(
                uid,
                &mut pwd,
                buf.as_mut_ptr().cast(),
                buf.len(),
                &mut found,
            )
        };
        if err == libc::ERANGE && buf.len() < 64 * 1024 {
            buf.resize(buf.len() * 4, 0);
            continue;
        }
        if err != 0 || found.is_null() || pwd.pw_name.is_null() {
            return None;
        }
        // SAFETY: on success pw_name points to a NUL-terminated string in `buf`.
        let name = unsafe { std::ffi::CStr::from_ptr(pwd.pw_name) };
        return name.to_str().ok().map(str::to_owned);
    }
}

/// Inside Flatpak: `getent` on the host, which sees the host's databases.
fn name_from_host(uid: u32) -> Option<String> {
    let output = Command::new("flatpak-spawn")
        .args(["--host", "getent", "passwd"])
        .arg(uid.to_string())
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    parse_passwd(&String::from_utf8_lossy(&output.stdout)).remove(&uid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_databases_name_root() {
        assert_eq!(name_from_nss(0).as_deref(), Some("root"));
    }

    #[test]
    fn passwd_names_come_first_and_a_fake_root_asks_nothing() {
        let root = SysRoot::at("tests/fixtures/desktop");
        let mut users = UserNames::new(
            "me:x:1000:1000::/home/me:/bin/sh\n",
            &root,
            ProcessSource::Direct,
        );
        assert_eq!(&*users.get(1000), "me");
        assert_eq!(&*users.get(0), "0");
    }

    #[test]
    fn the_real_machine_asks_its_user_databases() {
        let mut users = UserNames::new("", &SysRoot::host(), ProcessSource::Direct);
        // Not in the (empty) passwd text: looked up on the thread.
        let mut name = users.get(0);
        for _ in 0..20 {
            if &*name == "root" {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
            name = users.get(0);
        }
        assert_eq!(&*name, "root");
    }
}
