// ABOUTME: Sandbox providers: Docker (local or on an SSH host), plain SSH hosts, and Steel computers.
// ABOUTME: All work in a sandbox goes through "sh -c <script>", so one small interface serves all providers.

use crate::util::{run, sh_join, sh_quote};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::path::Path;
use std::process::{Command, Stdio};

/// Where the user wants the session to go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// Docker on this machine, or on an SSH host when `ssh_host` is set.
    Docker { ssh_host: Option<String> },
    /// An existing machine that beam reaches with ssh.
    Ssh { host: String },
    /// A new Steel computer, or a copy of a Steel checkpoint.
    Steel { checkpoint: Option<String> },
}

/// What a new sandbox needs to know.
pub struct CreateOpts<'a> {
    pub name: &'a str,
    pub image: &'a str,
    pub session_id: &'a str,
    pub timeout_secs: u64,
}

impl Target {
    pub fn parse(s: &str) -> Result<Target> {
        let host = |h: &str| -> Result<String> {
            let h = h.trim_end_matches('/');
            if h.is_empty() {
                bail!("target {s:?} has no host");
            }
            Ok(h.to_string())
        };
        if s == "docker" {
            Ok(Target::Docker { ssh_host: None })
        } else if let Some(h) = s.strip_prefix("docker+ssh://") {
            Ok(Target::Docker {
                ssh_host: Some(host(h)?),
            })
        } else if let Some(h) = s.strip_prefix("ssh://") {
            Ok(Target::Ssh { host: host(h)? })
        } else if s == "steel" {
            Ok(Target::Steel { checkpoint: None })
        } else if let Some(c) = s.strip_prefix("steel:") {
            if c.is_empty() {
                bail!("target {s:?} has no checkpoint id");
            }
            Ok(Target::Steel {
                checkpoint: Some(c.to_string()),
            })
        } else {
            bail!(
                "unknown target {s:?}. Use docker, docker+ssh://HOST, ssh://HOST, steel, or steel:CHECKPOINT"
            )
        }
    }

    /// Start a sandbox. For SSH, the host already exists, so this only checks that it answers.
    pub fn create(&self, o: &CreateOpts) -> Result<Sandbox> {
        let (name, image, session_id) = (o.name, o.image, o.session_id);
        match self {
            Target::Docker { ssh_host } => {
                let args = [
                    "docker",
                    "run",
                    "-d",
                    "--name",
                    name,
                    "--label",
                    "beam=1",
                    "--label",
                    &format!("beam.session={session_id}"),
                    "-u",
                    "0",
                    "--entrypoint",
                    "sleep",
                    image,
                    "infinity",
                ];
                let mut cmd = host_cmd(ssh_host.as_deref(), &args, false);
                run(&mut cmd).map_err(|e| {
                    if e.to_string().contains("Unable to find image") || e.to_string().contains("pull access denied") {
                        e.context(format!("image {image} is not there. Build it: docker build -t {image} images/base"))
                    } else {
                        e
                    }
                })?;
                Ok(Sandbox::Docker {
                    ssh_host: ssh_host.clone(),
                    container: name.to_string(),
                })
            }
            Target::Ssh { host } => {
                let sb = Sandbox::Ssh { host: host.clone() };
                sb.exec("true")
                    .with_context(|| format!("cannot reach {host} with ssh"))?;
                Ok(sb)
            }
            Target::Steel { checkpoint } => {
                let id = crate::steel::create(checkpoint.as_deref(), o.timeout_secs)?;
                let sb = Sandbox::Steel { id: id.clone() };
                if let Err(e) = crate::steel::exec(&id, crate::steel::BOOTSTRAP_SH) {
                    let _ = crate::steel::delete(&id);
                    return Err(e.context("cannot prepare the Steel computer"));
                }
                Ok(sb)
            }
        }
    }
}

/// A running sandbox. It is saved in the beam state file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Sandbox {
    Docker {
        ssh_host: Option<String>,
        container: String,
    },
    Ssh {
        host: String,
    },
    Steel {
        id: String,
    },
}

/// A command that runs `argv` on this machine, or on `ssh_host` through ssh.
fn host_cmd(ssh_host: Option<&str>, argv: &[&str], tty: bool) -> Command {
    match ssh_host {
        None => {
            let mut c = Command::new(argv[0]);
            c.args(&argv[1..]);
            c
        }
        Some(h) => {
            let mut c = Command::new("ssh");
            c.arg(if tty { "-t" } else { "-T" })
                .arg(h)
                .arg(sh_join(argv));
            c
        }
    }
}

impl Sandbox {
    pub fn describe(&self) -> String {
        match self {
            Sandbox::Docker {
                ssh_host: None,
                container,
            } => format!("docker:{container}"),
            Sandbox::Docker {
                ssh_host: Some(h),
                container,
            } => format!("docker+ssh://{h}:{container}"),
            Sandbox::Ssh { host } => format!("ssh://{host}"),
            Sandbox::Steel { id } => format!("steel:{id}"),
        }
    }

    fn command(&self, script: &str, stdin: bool, tty: bool) -> Command {
        match self {
            Sandbox::Docker {
                ssh_host,
                container,
            } => {
                let flag = match (stdin, tty) {
                    (_, true) => "-it",
                    (true, false) => "-i",
                    (false, false) => "",
                };
                let mut argv = vec!["docker", "exec"];
                if !flag.is_empty() {
                    argv.push(flag);
                }
                argv.extend([container.as_str(), "sh", "-c", script]);
                host_cmd(ssh_host.as_deref(), &argv, tty)
            }
            Sandbox::Ssh { host } => {
                let mut c = Command::new("ssh");
                c.arg(if tty { "-t" } else { "-T" })
                    .arg(host)
                    .arg(format!("sh -c {}", sh_quote(script)));
                c
            }
            Sandbox::Steel { id } => {
                // Only for display and tests: the Steel methods below do not use this command.
                let mut c = crate::steel::steel();
                c.args(["computer", "ssh", id, "--", "sh", "-c", script]);
                c
            }
        }
    }

    /// Make sure the sandbox can run commands (a Steel computer can pause itself).
    pub fn wake(&self) -> Result<()> {
        match self {
            Sandbox::Steel { id } => crate::steel::wake(id),
            _ => Ok(()),
        }
    }

    /// Run a script in the sandbox. Returns stdout.
    pub fn exec(&self, script: &str) -> Result<String> {
        if let Sandbox::Steel { id } = self {
            return crate::steel::exec(id, script);
        }
        let mut c = self.command(script, false, false);
        c.stdin(Stdio::null());
        run(&mut c)
    }

    /// Run a script with `data` on stdin.
    pub fn exec_input(&self, script: &str, data: &[u8]) -> Result<String> {
        use std::io::Write;
        if let Sandbox::Steel { id } = self {
            let tmp = crate::up::tempdir()?.join("stdin");
            std::fs::write(&tmp, data)?;
            let out = crate::steel::ssh_output(id, script, Stdio::from(File::open(&tmp)?));
            let _ = std::fs::remove_dir_all(tmp.parent().unwrap());
            return out.map(|o| String::from_utf8_lossy(&o).trim().to_string());
        }
        let mut c = self.command(script, true, false);
        c.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = c.spawn().context("cannot start the sandbox command")?;
        let mut stdin = child.stdin.take().context("no stdin")?;
        let data = data.to_vec();
        let writer = std::thread::spawn(move || stdin.write_all(&data));
        let out = child.wait_with_output()?;
        let _ = writer.join();
        if !out.status.success() {
            bail!(
                "sandbox command failed ({}): {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    /// Run a script with the content of a local file on stdin.
    pub fn exec_file(&self, script: &str, file: &Path) -> Result<String> {
        let f = File::open(file).with_context(|| format!("cannot open {}", file.display()))?;
        if let Sandbox::Steel { id } = self {
            let out = crate::steel::ssh_output(id, script, Stdio::from(f))?;
            return Ok(String::from_utf8_lossy(&out).trim().to_string());
        }
        let mut c = self.command(script, true, false);
        c.stdin(Stdio::from(f));
        run(&mut c)
    }

    /// Run a script and return stdout as raw bytes.
    pub fn exec_bytes(&self, script: &str) -> Result<Vec<u8>> {
        if let Sandbox::Steel { id } = self {
            return crate::steel::ssh_output(id, script, Stdio::null());
        }
        let mut c = self.command(script, false, false);
        c.stdin(Stdio::null());
        let out = c.output().context("cannot start the sandbox command")?;
        if !out.status.success() {
            bail!(
                "sandbox command failed ({}): {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(out.stdout)
    }

    /// Connect this terminal to a script in the sandbox (for example "tmux attach").
    pub fn interactive(&self, script: &str) -> Result<()> {
        if let Sandbox::Steel { id } = self {
            return crate::steel::interactive(id, script);
        }
        let status = self
            .command(script, true, true)
            .status()
            .context("cannot start the terminal")?;
        if !status.success() {
            bail!("the sandbox terminal stopped with {status}");
        }
        Ok(())
    }

    /// Delete the sandbox. For SSH hosts, `cleanup` removes what beam put there.
    pub fn destroy(&self, cleanup: &str) -> Result<()> {
        match self {
            Sandbox::Docker {
                ssh_host,
                container,
            } => run(&mut host_cmd(
                ssh_host.as_deref(),
                &["docker", "rm", "-f", container],
                false,
            ))
            .map(|_| ()),
            Sandbox::Ssh { .. } => self.exec(cleanup).map(|_| ()),
            Sandbox::Steel { id } => crate::steel::delete(id),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_targets() {
        assert_eq!(
            Target::parse("docker").unwrap(),
            Target::Docker { ssh_host: None }
        );
        assert_eq!(
            Target::parse("docker+ssh://agent").unwrap(),
            Target::Docker {
                ssh_host: Some("agent".into())
            }
        );
        assert_eq!(
            Target::parse("ssh://me@box/").unwrap(),
            Target::Ssh {
                host: "me@box".into()
            }
        );
        assert!(Target::parse("ssh://").is_err());
        assert_eq!(
            Target::parse("steel").unwrap(),
            Target::Steel { checkpoint: None }
        );
        assert_eq!(
            Target::parse("steel:ckp_1").unwrap(),
            Target::Steel {
                checkpoint: Some("ckp_1".into())
            }
        );
        assert!(Target::parse("steel:").is_err());
        assert!(Target::parse("e2b").is_err());
    }

    #[test]
    fn builds_docker_over_ssh_command() {
        let sb = Sandbox::Docker {
            ssh_host: Some("agent".into()),
            container: "beam-1".into(),
        };
        let c = sb.command("echo 'hi'", true, false);
        let args: Vec<String> = c
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert_eq!(c.get_program(), "ssh");
        assert_eq!(
            args,
            vec![
                "-T",
                "agent",
                r"docker exec -i beam-1 sh -c 'echo '\''hi'\'''"
            ]
        );
    }

    #[test]
    fn state_json_round_trip() {
        let sb = Sandbox::Ssh { host: "box".into() };
        let j = serde_json::to_string(&sb).unwrap();
        assert_eq!(j, r#"{"kind":"ssh","host":"box"}"#);
        assert_eq!(serde_json::from_str::<Sandbox>(&j).unwrap(), sb);
    }
}
