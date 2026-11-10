// SPDX-FileCopyrightText: 2026 oxlayer
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What this machine still lacks for the set to work.
//!
//! Spec: ../../specs/011-onboarding.md
//!
//! ## Why this exists
//!
//! Setting up a new machine for this set is a list of ~10 things,
//! different on each OS, and none of them gives a clear error when it is
//! missing: without an ACL on `/dev/hidraw` HID++ just finds no device;
//! without Screen Recording AVFoundation **hangs** instead of failing;
//! without an `/IT` task the Windows host starts in session 0 and
//! captures black. In all these cases the symptom is "it didn't work" and
//! the cause is three commands away.
//!
//! `status.sh` already answered this in the terminal, but only on Linux
//! and only for whoever knows it exists. This module is the same
//! diagnosis with three differences that matter: it runs on all three
//! machines, it runs **inside** the daemon (so it measures its own
//! session, not the SSH one -- see
//! [[../../.claude/rules/graphical-session]]), and it knows how to fix
//! part of what it finds.
//!
//! ## The honesty rule
//!
//! A step that cannot be measured becomes [`State::Error`], **never**
//! [`State::Ok`]. We already had two guards that guarded nothing
//! (`dispatch.backfill_gate` in another project, the langfuse `catch`)
//! and the pattern is always the same: silence looked like success. Here
//! "I don't know" is a visible state, and it blocks "ready" just like
//! "missing" does.
//!
//! After fixing, **measure again**. The button never turns green for
//! having run the command; it turns green for having measured the result
//! ([[../../.claude/rules/verify-what-arrived]]).
//!
//! ## What is pure and what is not
//!
//! This file is the catalog and the decision -- which steps exist on each
//! OS, in what order, who waits for whom, and when the set is ready.
//! Nothing here touches disk, network or processes, so the tests run on
//! the development laptop covering all **three** catalogs, not only this
//! machine's. That is what caught the dangling dependency in
//! `catalog(Windows)` before it existed on a machine.
//!
//! The real measurement and the repairs live in `setup_linux.rs`,
//! `setup_mac.rs` and `setup_win.rs`.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum System {
    Linux,
    Mac,
    Windows,
}

impl System {
    pub fn this_machine() -> Self {
        if cfg!(target_os = "macos") {
            System::Mac
        } else if cfg!(windows) {
            System::Windows
        } else {
            System::Linux
        }
    }
}

/// How this step gets fixed -- and who has to act.
///
/// The distinction between `Sudo` and `Click` is not cosmetic: the first
/// one the person solves by pasting a line into any terminal, even over
/// SSH; the second **only** works in front of the machine, because it is
/// macOS TCC or Windows UAC asking for human consent. Mixing the two is
/// promising automation that does not exist.
///
/// The wire names are the Rust names: `static/setup.html` reads
/// `kind`, `auto`, `label` and so on, so renaming a variant here changes
/// the page's contract and nothing else.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Fix {
    /// The daemon does it now, on its own, and measures again.
    Auto {
        label: &'static str,
    },
    /// Needs root. We show the exact line to copy.
    Sudo {
        line: &'static str,
    },
    /// Needs a human click in a system window. The daemon opens the
    /// right window; the person decides.
    Click {
        label: &'static str,
        location: &'static str,
    },
    /// There is no honest automation here. The recipe is written down.
    Recipe {
        line: &'static str,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct Step {
    pub id: &'static str,
    pub title: &'static str,
    /// One line saying what breaks without this.
    ///
    /// It exists because "enable this" without a reason becomes an
    /// automatic click, and three months later not even I remember why
    /// the step is on the list.
    pub why: &'static str,
    pub fix: Fix,
    /// Ids that must be `Ok` before this one makes sense.
    pub depends_on: &'static [&'static str],
    /// `false` = the set works without this, it only loses a part.
    pub essential: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Ok,
    Missing,
    /// Measuring failed. It is not "ok" and not "missing": it is "I don't
    /// know".
    Error,
    /// Does not apply to this machine (no GPU, no receiver).
    NotApplicable,
}

#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub id: String,
    pub state: State,
    pub detail: String,
}

impl Check {
    pub fn ok(id: &str, detail: impl Into<String>) -> Self {
        Check { id: id.into(), state: State::Ok, detail: detail.into() }
    }
    pub fn missing(id: &str, detail: impl Into<String>) -> Self {
        Check { id: id.into(), state: State::Missing, detail: detail.into() }
    }
    pub fn error(id: &str, detail: impl Into<String>) -> Self {
        Check { id: id.into(), state: State::Error, detail: detail.into() }
    }
    pub fn not_applicable(id: &str, detail: impl Into<String>) -> Self {
        Check { id: id.into(), state: State::NotApplicable, detail: detail.into() }
    }
    /// Shortcut for a boolean measurement, which is most of them.
    pub fn when(id: &str, cond: bool, good: impl Into<String>, bad: impl Into<String>) -> Self {
        if cond {
            Check::ok(id, good)
        } else {
            Check::missing(id, bad)
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    /// Essential steps that apply to this machine.
    pub total: usize,
    pub done: usize,
    /// Every applicable essential step is `Ok`.
    pub ready: bool,
    /// First step where acting makes a difference now.
    pub next: Option<String>,
    /// Steps that fail but depend on another one that fails earlier.
    pub waiting: Vec<String>,
}

pub fn state_of(checks: &[Check], id: &str) -> State {
    // Absent = Error, not Ok. A step nobody measured did not pass:
    // forgetting to measure is exactly how a "ready" lies.
    checks.iter().find(|m| m.id == id).map(|m| m.state).unwrap_or(State::Error)
}

/// Which dependency of this step is not up yet.
///
/// Returns the **first** one -- catalog order is resolution order, so
/// the first is the one the person should tackle.
pub fn blocked_by(p: &Step, checks: &[Check]) -> Option<String> {
    p.depends_on
        .iter()
        .find(|d| !matches!(state_of(checks, d), State::Ok | State::NotApplicable))
        .map(|d| (*d).to_string())
}

pub fn summarize(steps: &[Step], checks: &[Check]) -> Summary {
    let mut total = 0;
    let mut done = 0;
    let mut ready = true;
    let mut next = None;
    let mut waiting = Vec::new();

    for p in steps {
        let e = state_of(checks, p.id);
        if e == State::NotApplicable {
            // Does not apply: it neither counts nor delays anyone.
            // Counting it as pending would make the panel complain about
            // a missing GPU on a machine that will never have one.
            continue;
        }
        if p.essential {
            total += 1;
            if e == State::Ok {
                done += 1;
            } else {
                ready = false;
            }
        }
        if e != State::Ok {
            match blocked_by(p, checks) {
                Some(_) => waiting.push(p.id.to_string()),
                None if next.is_none() => next = Some(p.id.to_string()),
                None => {}
            }
        }
    }

    Summary { total, done, ready, next, waiting }
}

// ---------------------------------------------------------------- catalogs

/// This machine's steps, in resolution order.
pub fn local_catalog() -> Vec<Step> {
    catalog(System::this_machine())
}

pub fn catalog(s: System) -> Vec<Step> {
    match s {
        System::Linux => linux(),
        System::Mac => mac(),
        System::Windows => windows(),
    }
}

fn linux() -> Vec<Step> {
    vec![
        Step {
            id: "binaries",
            title: "Binaries installed",
            why: "the daemon, the screen host and the KVM live in \
                     ~/.local/bin. oxdesk-kvm went missing once and the tray \
                     button failed with \"could not execute\" -- compiled, \
                     never installed.",
            fix: Fix::Recipe { line: "bash install.sh   # or ./scripts/deploy.sh" },
            depends_on: &[],
            essential: true,
        },
        Step {
            id: "config",
            title: "This machine's config",
            why: "every machine has its own config and nothing syncs it. \
                     Without this machine's entry marked as local, the panel \
                     does not know who \"here\" is.",
            fix: Fix::Auto { label: "Write the default config" },
            depends_on: &[],
            essential: true,
        },
        Step {
            id: "daemon",
            title: "User service running",
            why: "oxdesk.service is what starts the panel at login. Running \
                     it by hand works until the first reboot.",
            fix: Fix::Auto { label: "Enable and start" },
            depends_on: &["binaries"],
            essential: true,
        },
        Step {
            id: "receiver",
            title: "Access to the receiver (HID++)",
            why: "/dev/hidraw is how we switch the keyboard's and the \
                     mouse's Easy-Switch channel. Without access the panel \
                     simply finds no device -- it does not say \"permission \
                     denied\".",
            fix: Fix::Sudo {
                line: "# from the repo root:\n\
                        sudo install -m644 oxdesk/udev/70-oxdesk-hidraw.rules /etc/udev/rules.d/\n\
                        sudo udevadm control --reload\n\
                        sudo udevadm trigger --subsystem-match=hidraw --action=add",
            },
            depends_on: &[],
            essential: false,
        },
        Step {
            id: "input-kvm",
            title: "Access to keyboard and mouse (KVM over LAN)",
            why: "the KVM reads /dev/input/event* and calls EVIOCGRAB. The \
                     rule uses TAG+=\"uaccess\" restricted to vendor 046d, and \
                     NOT the input group: the group would give every process \
                     on the account permanent access to every input device -- \
                     a passive keylogger for free.",
            fix: Fix::Sudo {
                line: "# from the repo root:\n\
                        sudo install -m644 oxdesk/udev/70-oxdesk-input.rules /etc/udev/rules.d/\n\
                        sudo udevadm control --reload\n\
                        sudo udevadm trigger --subsystem-match=input",
            },
            depends_on: &[],
            essential: false,
        },
        Step {
            id: "wayland",
            title: "Wayland session",
            why: "edge detection uses Mutter's private API \
                     (org.gnome.Mutter.InputCapture), and the clipboard uses \
                     the same Mutter's RemoteDesktop. Neither has been \
                     validated on X11.",
            fix: Fix::Recipe {
                line: "pick \"GNOME\" (not \"GNOME on Xorg\") on the login screen",
            },
            depends_on: &[],
            essential: false,
        },
        Step {
            id: "clipboard",
            title: "Clipboard across machines",
            why: "Mutter has to accept the RemoteDesktop session for us to \
                     watch the clipboard. Without it a copy does not cross.",
            fix: Fix::Auto { label: "Test it now" },
            depends_on: &["daemon"],
            essential: true,
        },
        Step {
            id: "peers",
            title: "SSH reaches the other machines",
            why: "SSH carries everything that crosses a machine -- screen \
                     switching, clipboard, files. Without an authorized key \
                     the panel shows the others as dead.",
            fix: Fix::Recipe {
                line: "ssh-copy-id <alias>   # alias is each host's `ssh` field in the config",
            },
            depends_on: &["config"],
            essential: true,
        },
        Step {
            id: "nvenc",
            title: "Hardware video encoder",
            why: "this machine can only be WATCHED if it has NVENC. Without \
                     it the panel falls back to the software path, which is \
                     slower and runs hot.",
            fix: Fix::Recipe {
                line: "install the proprietary NVIDIA driver; check with nvidia-smi",
            },
            depends_on: &["binaries"],
            essential: false,
        },
        Step {
            id: "tray",
            title: "Tray icon",
            why: "it is the shortest way to bring the keyboard back when it \
                     is on another machine. Without it you depend on \
                     remembering the double Ctrl.",
            fix: Fix::Auto { label: "Turn the tray on" },
            depends_on: &["daemon"],
            essential: false,
        },
        Step {
            id: "ota",
            title: "Automatic updates (OTA)",
            why: "this machine updates itself from GitHub Releases -- it is \
                     what keeps a travelling machine current when nobody can \
                     run the deploy. Without the timer, the old build stays.",
            fix: Fix::Auto { label: "Install the timer and check now" },
            depends_on: &["binaries"],
            essential: false,
        },
    ]
}

fn mac() -> Vec<Step> {
    vec![
        Step {
            id: "binaries",
            title: "Binaries installed",
            why: "the Mac is both a client AND a screen host. Leaving the \
                     host behind has happened: the machine watched the others \
                     and was never watched itself, and the panel fell back to \
                     VNC without saying why.",
            fix: Fix::Recipe {
                line: "./scripts/deploy.sh   # builds on the Mac itself, no cross-compile",
            },
            depends_on: &[],
            essential: true,
        },
        Step {
            id: "config",
            title: "This machine's config",
            why: "every machine has its own config and nothing syncs it.",
            fix: Fix::Auto { label: "Write the default config" },
            depends_on: &[],
            essential: true,
        },
        Step {
            id: "daemon",
            title: "Daemon LaunchAgent",
            why: "oxdesk has to start inside the graphical session. Launched \
                     over SSH it sees neither the keychain nor the \
                     permissions.",
            fix: Fix::Auto { label: "Install and load" },
            depends_on: &["binaries"],
            essential: true,
        },
        Step {
            id: "ffmpeg",
            title: "The ffmpeg the binaries ask for",
            why: "the Mac is the only machine that links ffmpeg -- capture \
                     through avfoundation, encoding through \
                     h264_videotoolbox -- and the soname goes INTO THE \
                     BINARY: built against major 9 it will not load against \
                     an installed major 7. With the wrong formula the screen \
                     does not come up and the OTA refuses the whole update: \
                     correct, but it arrives as an hourly refusal instead of \
                     as one line here.",
            fix: Fix::Recipe { line: "brew install ffmpeg@7" },
            depends_on: &["binaries"],
            essential: true,
        },
        Step {
            id: "identity",
            title: "Local signing identity",
            why: "without it the binary is ad-hoc, TCC binds the grant to \
                     the cdhash, and screen permission DIES on every build. \
                     Measured: grant it, run a deploy, and watch the host \
                     hang again with no error anywhere in the log.",
            fix: Fix::Recipe {
                line: "bash ~/.local/bin/create-identity.sh   # in the Mac's Terminal: over SSH the keychain is locked",
            },
            depends_on: &[],
            essential: true,
        },
        Step {
            id: "signature",
            title: "Screen host signed",
            why: "this is what makes the grant survive the next deploy. \
                     Granting permission BEFORE signing wastes the grant -- \
                     and only the machine's owner can give it back, by hand.",
            fix: Fix::Auto { label: "Sign it now" },
            depends_on: &["binaries", "identity"],
            essential: true,
        },
        Step {
            id: "screen-recording",
            title: "Screen Recording permission",
            why: "without it AVFoundation HANGS instead of failing: the log \
                     stops at \"opening display 1\" and the last line is an \
                     ffmpeg message about pixel format that has nothing to do \
                     with the problem. Careful: the FIRST open after swapping \
                     the binary took ~30 s, with the log stuck on that same \
                     sentence (later ones, ~3 s). Wait 30 s before blaming \
                     TCC.",
            fix: Fix::Click {
                label: "Ask for the permission",
                location: "Settings > Privacy & Security > Screen Recording > oxdesk-host",
            },
            depends_on: &["signature"],
            essential: true,
        },
        Step {
            id: "accessibility",
            title: "Accessibility permission",
            why: "this is what allows injecting keyboard and mouse. Without \
                     it the screen appears and the pointer does not move -- \
                     same \"it didn't work\" symptom, different cause.",
            fix: Fix::Click {
                label: "Ask for the permission",
                location: "Settings > Privacy & Security > Accessibility > oxdesk-host",
            },
            depends_on: &["signature"],
            essential: true,
        },
        Step {
            id: "launchagent",
            title: "Screen host LaunchAgent",
            why: "the host starts through launchctl inside the graphical \
                     session, never over SSH. RunAtLoad is false on purpose: \
                     capturing with nobody watching would waste battery.",
            fix: Fix::Auto { label: "Install and load" },
            depends_on: &["binaries"],
            essential: true,
        },
        Step {
            id: "display",
            title: "A display to capture",
            why: "with no display attached macOS has no framebuffer, and \
                     the screen host cannot serve a frame however many \
                     permissions it holds. Measured on a MacBook whose panel \
                     broke: zero active displays, every other step green, \
                     and nothing on this page said why the screen was dead. \
                     Unlike Windows there is no software display here -- an \
                     external monitor, or a cheap HDMI dummy plug, is what \
                     brings one back.",
            fix: Fix::Recipe {
                line: "attach a monitor (or an HDMI display emulator) -- there is no software fix on macOS",
            },
            depends_on: &[],
            essential: false,
        },
        Step {
            id: "clipboard",
            title: "Clipboard across machines",
            why: "reading the Mac's clipboard goes through JXA, and that \
                     only runs inside the graphical session.",
            fix: Fix::Auto { label: "Test it now" },
            depends_on: &["daemon"],
            essential: true,
        },
        Step {
            id: "remote-login",
            title: "Remote Login (inbound SSH)",
            why: "this is what lets the deploy and the panel reach this Mac. \
                     Without it the machine only talks, never listens.",
            fix: Fix::Sudo { line: "sudo systemsetup -f -setremotelogin on" },
            depends_on: &[],
            essential: true,
        },
        Step {
            id: "tailscale",
            title: "Tailscale (off the LAN)",
            why: "this is what reaches the other machines from outside the \
                     house. Install it through brew, with its own service, \
                     and never through the app: the app's network extension, \
                     without a login, took the Mac's internet down.",
            fix: Fix::Click {
                label: "Install through brew",
                location: "the Terminal that will open: type the password and open the login link",
            },
            depends_on: &[],
            essential: false,
        },
        Step {
            id: "peers",
            title: "SSH reaches the other machines",
            why: "SSH carries everything that crosses a machine.",
            fix: Fix::Recipe { line: "ssh-copy-id <alias>" },
            depends_on: &["config"],
            essential: true,
        },
        Step {
            id: "ota",
            title: "Automatic updates (OTA)",
            why: "this is the step that keeps a travelling Mac current. The \
                     agent downloads the published build, **signs it with the \
                     local identity** and only then swaps it -- which is what \
                     preserves Screen Recording, and what an ad-hoc binary \
                     from CI would lose.",
            fix: Fix::Auto { label: "Install the timer and check now" },
            depends_on: &["binaries", "identity"],
            essential: false,
        },
    ]
}

fn windows() -> Vec<Step> {
    vec![
        Step {
            id: "binaries",
            title: "Binaries installed",
            why: "the daemon, the agent, the host and the viewer live in \
                     %LOCALAPPDATA%\\oxdesk. The deploy signs each one -- \
                     unsigned, Defender eats the executable without a word.",
            fix: Fix::Recipe {
                line: "./scripts/deploy.sh   # from Linux: cross-compiles, signs and swaps",
            },
            depends_on: &[],
            essential: true,
        },
        Step {
            id: "config",
            title: "This machine's config",
            why: "every machine has its own config and nothing syncs it.",
            fix: Fix::Auto { label: "Write the default config" },
            depends_on: &[],
            essential: true,
        },
        Step {
            id: "daemon",
            title: "Daemon logon task",
            why: "on Windows oxdesk starts from a scheduled task with /IT, \
                     not from a service: a service is born in session 0, and \
                     from there it cannot see keyboard, mouse or clipboard.",
            fix: Fix::Auto { label: "Create the task" },
            depends_on: &["binaries"],
            essential: true,
        },
        Step {
            id: "screen-task",
            title: "Interactive task for the screen host",
            why: "same boundary: without /IT, DXGI returns E_ACCESSDENIED or \
                     captures black, and the symptom says nothing about \
                     sessions.",
            fix: Fix::Auto { label: "Create the task" },
            depends_on: &["binaries"],
            essential: true,
        },
        Step {
            id: "clipboard",
            title: "Clipboard across machines",
            why: "reading the clipboard requires being in the interactive \
                     session. Over SSH you read session 0, which is always \
                     empty -- and \"empty\" looks like \"nothing copied\".",
            fix: Fix::Auto { label: "Test it now" },
            depends_on: &["daemon"],
            essential: true,
        },
        Step {
            id: "sshd",
            title: "OpenSSH server",
            why: "it is how the other machines send deploys, clipboard and \
                     files. Without it this machine disappears from the \
                     panel.",
            fix: Fix::Sudo {
                line: "Add-WindowsCapability -Online -Name OpenSSH.Server~~~~0.0.1.0; Set-Service sshd -StartupType Automatic; Start-Service sshd",
            },
            depends_on: &[],
            essential: true,
        },
        Step {
            id: "admin-key",
            title: "Authorized key",
            why: "Windows OpenSSH reads an administrator's key from \
                     ProgramData\\ssh\\administrators_authorized_keys, NOT \
                     from the user's ~/.ssh. Putting it in the wrong place \
                     fails with \"Permission denied\" and no hint at all.",
            fix: Fix::Recipe {
                line: "ssh-copy-id does not exist here: see .claude/rules/remote-powershell.md",
            },
            depends_on: &["sshd"],
            essential: true,
        },
        Step {
            id: "peers",
            title: "SSH reaches the other machines",
            why: "SSH carries everything that crosses a machine.",
            fix: Fix::Recipe {
                line: "ssh-keygen, then copy the public key to the other side",
            },
            depends_on: &["config"],
            essential: true,
        },
        Step {
            id: "virtual-display",
            title: "A screen with no monitor attached",
            why: "with no monitor Windows has nothing to capture: the viewer \
                     connects, the click goes through and no frame ever \
                     arrives. A virtual display keeps access to a machine \
                     sitting on its own.",
            fix: Fix::Click {
                label: "Install the virtual display",
                location: "the Windows administrator prompt",
            },
            depends_on: &[],
            essential: false,
        },
        Step {
            id: "screen-lock",
            title: "Screen saver and lock",
            why: "with the session locked DXGI returns E_ACCESSDENIED and \
                     the remote screen dies on its own after a few minutes. \
                     The choice is yours: turn the lock off, or know that \
                     the screen drops when it kicks in.",
            fix: Fix::Recipe {
                line: "powercfg /setacvalueindex SCHEME_CURRENT SUB_NONE CONSOLELOCK 0",
            },
            depends_on: &[],
            essential: false,
        },
        Step {
            id: "ota",
            title: "Automatic updates (OTA)",
            why: "here the OTA **only checks and reports**: the Authenticode \
                     key never leaves the build machine, and an `.exe` without \
                     a valid signature is what Defender deletes as it is \
                     written. It exists so the panel can say when this \
                     machine fell behind -- the deploy is what swaps it.",
            fix: Fix::Auto { label: "Install the task and check now" },
            depends_on: &["binaries"],
            essential: false,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_THREE: [System; 3] = [System::Linux, System::Mac, System::Windows];

    fn m(id: &str, e: State) -> Check {
        Check { id: id.into(), state: e, detail: String::new() }
    }

    #[test]
    fn error_does_not_count_as_ok() {
        // The point of the honesty rule: "I can't measure" must weigh the
        // same as "missing". If it weighed less, the hardest step to
        // measure would be the easiest to pass.
        let steps = linux();
        let checks: Vec<_> = steps.iter().map(|p| m(p.id, State::Error)).collect();
        let r = summarize(&steps, &checks);
        assert!(!r.ready);
        assert_eq!(r.done, 0);
    }

    #[test]
    fn step_without_check_does_not_pass() {
        // Forgetting to measure a step is the easiest way for a "ready"
        // to lie -- and the most likely one, because the catalog and the
        // measurement live in different files.
        let steps = linux();
        let checks: Vec<_> = steps
            .iter()
            .filter(|p| p.id != "clipboard")
            .map(|p| m(p.id, State::Ok))
            .collect();
        let r = summarize(&steps, &checks);
        assert!(!r.ready);
        assert_eq!(state_of(&checks, "clipboard"), State::Error);
    }

    #[test]
    fn not_applicable_neither_blocks_nor_counts() {
        let steps = linux();
        let total_before = {
            let all: Vec<_> = steps.iter().map(|p| m(p.id, State::Ok)).collect();
            summarize(&steps, &all).total
        };
        let checks: Vec<_> = steps
            .iter()
            .map(|p| m(p.id, if p.id == "clipboard" { State::NotApplicable } else { State::Ok }))
            .collect();
        let r = summarize(&steps, &checks);
        assert!(r.ready, "an out-of-scope step does not prevent ready");
        assert_eq!(r.total, total_before - 1, "and it does not count either");
        assert_eq!(r.next, None);
    }

    #[test]
    fn non_essential_does_not_prevent_ready() {
        let steps = linux();
        let checks: Vec<_> = steps
            .iter()
            .map(|p| m(p.id, if p.essential { State::Ok } else { State::Missing }))
            .collect();
        let r = summarize(&steps, &checks);
        assert!(r.ready);
        // But it still shows up as the next action: not blocking is not
        // hiding.
        assert!(r.next.is_some());
    }

    #[test]
    fn next_skips_steps_waiting_on_a_dependency() {
        // `daemon` depends on `binaries`. With both missing, the offered
        // action must be installing the binaries -- telling the person to
        // enable a service whose executable does not exist fixes nothing
        // and teaches them to distrust the wizard.
        let steps = linux();
        let checks: Vec<_> = steps
            .iter()
            .map(|p| {
                let missing = p.id == "daemon" || p.id == "binaries";
                m(p.id, if missing { State::Missing } else { State::Ok })
            })
            .collect();
        let r = summarize(&steps, &checks);
        assert_eq!(r.next.as_deref(), Some("binaries"));
        assert!(r.waiting.contains(&"daemon".to_string()));
    }

    #[test]
    fn out_of_scope_dependency_releases_its_dependents() {
        // `NotApplicable` is not a failure: if the dependency does not
        // apply to this machine, whoever waits for it must be able to move.
        let p = Step {
            id: "x",
            title: "",
            why: "",
            fix: Fix::Auto { label: "" },
            depends_on: &["y"],
            essential: true,
        };
        assert_eq!(blocked_by(&p, &[m("y", State::NotApplicable)]), None);
        assert_eq!(blocked_by(&p, &[m("y", State::Missing)]).as_deref(), Some("y"));
    }

    #[test]
    fn all_ok_is_ready() {
        for s in ALL_THREE {
            let steps = catalog(s);
            let checks: Vec<_> = steps.iter().map(|p| m(p.id, State::Ok)).collect();
            let r = summarize(&steps, &checks);
            assert!(r.ready, "{s:?}");
            assert_eq!(r.next, None, "{s:?}");
            assert_eq!(r.done, r.total, "{s:?}");
            assert!(r.total > 0, "{s:?}");
        }
    }

    #[test]
    fn ids_unique_in_each_catalog() {
        for s in ALL_THREE {
            let steps = catalog(s);
            let mut ids: Vec<_> = steps.iter().map(|p| p.id).collect();
            let before = ids.len();
            ids.sort_unstable();
            ids.dedup();
            assert_eq!(ids.len(), before, "duplicate id in {s:?}");
        }
    }

    #[test]
    fn dependency_exists_and_comes_first() {
        // A dangling dependency would leave the step waiting forever, and
        // the wizard would never offer any action -- a "this is missing"
        // screen with no button. A dependency pointing FORWARD would show
        // "waiting" above the thing waited for, which reads bottom-up.
        for s in ALL_THREE {
            let steps = catalog(s);
            for (i, p) in steps.iter().enumerate() {
                for d in p.depends_on {
                    let pos = steps.iter().position(|q| q.id == *d);
                    let pos = pos.unwrap_or_else(|| panic!("{s:?}: {} depends on {d}, which does not exist", p.id));
                    assert!(pos < i, "{s:?}: {} depends on {d}, which comes later", p.id);
                }
            }
        }
    }

    #[test]
    fn every_step_says_why() {
        // A step without a reason becomes an automatic click. Cheap to
        // test, and the test is what stops haste from slipping in a
        // `why: ""`.
        for s in ALL_THREE {
            for p in catalog(s) {
                assert!(p.why.len() > 40, "{s:?}: {} has no written reason", p.id);
                assert!(!p.title.is_empty(), "{s:?}: {} has no title", p.id);
            }
        }
    }
}

// ---------------------------------------------------------- per-OS dispatch
//
// Only here does the pure module touch the world, and only by
// delegation: the real measurement lives in the platform file. The
// alternative -- `cfg` scattered through the catalog -- would make the
// tests above cover only the OS being compiled, which is exactly what
// they exist to avoid.

/// Measures this machine's steps, in catalog order.
pub fn measure(cfg: &crate::config::Config) -> Vec<Check> {
    #[cfg(target_os = "linux")]
    return crate::setup_linux::measure(cfg);
    #[cfg(target_os = "macos")]
    return crate::setup_mac::measure(cfg);
    #[cfg(windows)]
    return crate::setup_win::measure(cfg);
}

/// Tries to fix a step and returns **what it measured afterwards**.

pub fn repair(id: &str, cfg: &crate::config::Config) -> Result<String, String> {
    #[cfg(target_os = "linux")]
    return crate::setup_linux::repair(id, cfg);
    #[cfg(target_os = "macos")]
    return crate::setup_mac::repair(id, cfg);
    #[cfg(windows)]
    return crate::setup_win::repair(id, cfg);
}
