//! Command-line parsing.
//!
//! Hand-rolled to keep the zero-extra-dependency posture, but structured:
//! a pure function from argv to a `Cmd`, with its own tests. The previous
//! inline version had real defects -- config was loaded (and a default
//! `robot.toml` WRITTEN) before dispatch, so `robotd restore … --into
//! /Volumes/stick` littered a config into the current directory; flags were
//! scanned across the whole argv, so `--config` could bind twice; a
//! subcommand was only recognised at argv[1], so `robotd --config x backup`
//! silently started a server; and an unknown subcommand booted the daemon.

use std::path::PathBuf;

pub const USAGE: &str = "\
bender -- labs robot v0.2

usage:
  robotd [--config <file>]                      run the robot (default)
  robotd eval [--live] [--config <file>]        run the eval suite
  robotd notify <text> [--config <file>]        put a notice in the owner's chat
  robotd backup [--config <file>]               write a sealed backup
  robotd backup-restore <sealed> <dir> [--config <file>]
  robotd package [<dest.pkg>] [--config <file>] export the robot package
  robotd restore <pkg> --code <code> --into <dir> [--port <n>] [--force]
  robotd --help | --version

notes:
  the robot package is sealed with a one-time code printed at export;
  carry it separately from the file. restore refuses to overwrite an
  existing robot unless --force, and then moves the old data aside
  rather than deleting it.
";

#[derive(Debug, PartialEq, Eq)]
pub enum Cmd {
    Serve {
        config: PathBuf,
    },
    Eval {
        config: PathBuf,
        live: bool,
    },
    Notify {
        config: PathBuf,
        text: String,
    },
    Backup {
        config: PathBuf,
    },
    BackupRestore {
        config: PathBuf,
        sealed: PathBuf,
        dest: PathBuf,
    },
    Package {
        config: PathBuf,
        dest: Option<PathBuf>,
    },
    Restore {
        pkg: PathBuf,
        code: String,
        into: PathBuf,
        port: u16,
        force: bool,
    },
    Help,
    Version,
}

fn default_config() -> PathBuf {
    PathBuf::from("robot.toml")
}

/// Pull `--flag <value>` out of `args`, returning the value. Errors if the
/// flag is present without a value or given twice.
fn take_value(args: &mut Vec<String>, flag: &str) -> Result<Option<String>, String> {
    let mut found = None;
    let mut i = 0;
    while i < args.len() {
        if args[i] == flag {
            if found.is_some() {
                return Err(format!("{flag} given more than once"));
            }
            if i + 1 >= args.len() {
                return Err(format!("{flag} needs a value"));
            }
            found = Some(args[i + 1].clone());
            args.drain(i..i + 2);
        } else {
            i += 1;
        }
    }
    Ok(found)
}

fn take_flag(args: &mut Vec<String>, flag: &str) -> bool {
    let before = args.len();
    args.retain(|a| a != flag);
    args.len() != before
}

/// Parse argv (excluding argv[0]).
pub fn parse(argv: &[String]) -> Result<Cmd, String> {
    let mut args: Vec<String> = argv.to_vec();

    if take_flag(&mut args, "--help") || take_flag(&mut args, "-h") {
        return Ok(Cmd::Help);
    }
    if take_flag(&mut args, "--version") || take_flag(&mut args, "-V") {
        return Ok(Cmd::Version);
    }

    // global flag: valid before or after the subcommand
    let config = take_value(&mut args, "--config")?
        .map(PathBuf::from)
        .unwrap_or_else(default_config);

    let sub = args.first().cloned();
    let rest: Vec<String> = args.iter().skip(1).cloned().collect();

    match sub.as_deref() {
        None => Ok(Cmd::Serve { config }),
        Some("serve") | Some("run") => Ok(Cmd::Serve { config }),
        Some("eval") => {
            let mut rest = rest;
            let live = take_flag(&mut rest, "--live");
            reject_extra(&rest, "eval")?;
            Ok(Cmd::Eval { config, live })
        }
        Some("notify") => {
            if rest.len() != 1 {
                return Err("notify needs exactly one <text> argument (quote it)".into());
            }
            Ok(Cmd::Notify {
                config,
                text: rest[0].clone(),
            })
        }
        Some("backup") => {
            reject_extra(&rest, "backup")?;
            Ok(Cmd::Backup { config })
        }
        Some("backup-restore") => {
            if rest.len() != 2 {
                return Err("backup-restore needs <sealed> <dest-dir>".into());
            }
            Ok(Cmd::BackupRestore {
                config,
                sealed: PathBuf::from(&rest[0]),
                dest: PathBuf::from(&rest[1]),
            })
        }
        Some("package") => {
            if rest.len() > 1 {
                return Err("package takes at most one destination path".into());
            }
            Ok(Cmd::Package {
                config,
                dest: rest.first().map(PathBuf::from),
            })
        }
        Some("restore") => {
            let mut rest = rest;
            let force = take_flag(&mut rest, "--force");
            let code = take_value(&mut rest, "--code")?
                .ok_or("restore needs --code <code> (printed when the package was made)")?;
            let into = take_value(&mut rest, "--into")?
                .ok_or("restore needs --into <dir>")?;
            let port: u16 = match take_value(&mut rest, "--port")? {
                Some(p) => p.parse().map_err(|_| format!("--port {p} is not a port"))?,
                None => 7778,
            };
            if rest.len() != 1 {
                return Err("restore needs exactly one <pkg> path".into());
            }
            Ok(Cmd::Restore {
                pkg: PathBuf::from(&rest[0]),
                code,
                into: PathBuf::from(into),
                port,
                force,
            })
        }
        Some(other) => Err(format!(
            "unknown command '{other}'. run `robotd --help` for usage."
        )),
    }
}

fn reject_extra(rest: &[String], cmd: &str) -> Result<(), String> {
    if rest.is_empty() {
        Ok(())
    } else {
        Err(format!("{cmd} takes no extra arguments (got {rest:?})"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> Result<Cmd, String> {
        parse(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn no_args_serves_with_the_default_config() {
        assert_eq!(
            p(&[]).unwrap(),
            Cmd::Serve {
                config: PathBuf::from("robot.toml")
            }
        );
    }

    /// Regression: the subcommand was only read at argv[1], so a global
    /// flag in front of it silently started a web server instead.
    #[test]
    fn global_config_works_before_and_after_the_subcommand() {
        let a = p(&["--config", "other.toml", "backup"]).unwrap();
        let b = p(&["backup", "--config", "other.toml"]).unwrap();
        assert_eq!(a, b);
        assert_eq!(
            a,
            Cmd::Backup {
                config: PathBuf::from("other.toml")
            }
        );
    }

    /// Regression: an unknown subcommand used to fall through and boot the
    /// daemon -- `robotd bakup` started a server on port 7777.
    #[test]
    fn unknown_commands_are_errors_not_a_silent_server() {
        let e = p(&["bakup"]).unwrap_err();
        assert!(e.contains("unknown command"), "{e}");
    }

    #[test]
    fn restore_requires_its_flags_and_parses_them() {
        assert!(p(&["restore", "a.pkg"]).unwrap_err().contains("--code"));
        assert!(p(&["restore", "a.pkg", "--code", "x"])
            .unwrap_err()
            .contains("--into"));
        assert_eq!(
            p(&[
                "restore", "a.pkg", "--code", "abc", "--into", "/tmp/usb", "--port", "7900",
                "--force"
            ])
            .unwrap(),
            Cmd::Restore {
                pkg: PathBuf::from("a.pkg"),
                code: "abc".into(),
                into: PathBuf::from("/tmp/usb"),
                port: 7900,
                force: true,
            }
        );
        // a bad port is an error, not a silent default
        assert!(p(&[
            "restore", "a.pkg", "--code", "c", "--into", "/x", "--port", "nope"
        ])
        .unwrap_err()
        .contains("not a port"));
    }

    /// Regression: flags were scanned across the whole argv, so a path that
    /// looked like a flag value could bind twice.
    #[test]
    fn duplicate_flags_are_rejected() {
        let e = p(&["--config", "a.toml", "backup", "--config", "b.toml"]).unwrap_err();
        assert!(e.contains("more than once"), "{e}");
    }

    #[test]
    fn eval_live_and_stray_arguments() {
        assert_eq!(
            p(&["eval", "--live"]).unwrap(),
            Cmd::Eval {
                config: PathBuf::from("robot.toml"),
                live: true
            }
        );
        assert!(p(&["eval", "oops"]).unwrap_err().contains("no extra"));
    }

    #[test]
    fn help_and_version_win_over_everything() {
        assert_eq!(p(&["--help"]).unwrap(), Cmd::Help);
        assert_eq!(p(&["backup", "--version"]).unwrap(), Cmd::Version);
    }

    #[test]
    fn notify_takes_one_quoted_argument() {
        assert_eq!(
            p(&["notify", "backup failed"]).unwrap(),
            Cmd::Notify {
                config: PathBuf::from("robot.toml"),
                text: "backup failed".into()
            }
        );
        assert!(p(&["notify"]).unwrap_err().contains("exactly one"));
        assert!(p(&["notify", "a", "b"]).unwrap_err().contains("exactly one"));
    }

    #[test]
    fn package_destination_is_optional() {
        assert_eq!(
            p(&["package"]).unwrap(),
            Cmd::Package {
                config: PathBuf::from("robot.toml"),
                dest: None
            }
        );
        assert!(matches!(
            p(&["package", "out.pkg"]).unwrap(),
            Cmd::Package { dest: Some(_), .. }
        ));
    }
}
