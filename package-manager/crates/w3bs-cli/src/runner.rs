//! `w3bs resource run --via`: hand rendered instructions to a model runtime.
//!
//! The template runtime never calls a model (W3BS-PROMPT-1). A runner is the separately
//! authorized runtime the spec leaves to integrators: a peer CLI that W3BS installs and
//! drives, never a provider SDK compiled in here. The rendered text goes to its stdin.

use crate::link;
use serde_json::{Value, json};
use std::io::Write;
use std::process::{Command, Stdio};
use w3bs_core::Error;

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq)]
pub enum Via {
    /// `moshcode oneshot <engine> -` (claude, codex, gemini, zai, perplexity, …)
    Moshcode,
    /// `infernet chat`: the Infernet P2P network, or its local engine
    Infernet,
    /// `c0mpute infernet chat`: Infernet through the c0mpute node
    C0mpute,
}

impl Via {
    fn package(self) -> &'static str {
        match self {
            Via::Moshcode => "moshcode.sh/moshcode",
            Via::Infernet => "infernetprotocol.com/infernet",
            Via::C0mpute => "c0mpute.com/c0mpute",
        }
    }
}

/// The program and arguments for a runner. `model` is the engine for moshcode.
pub fn command(via: Via, model: Option<&str>) -> Result<(&'static str, Vec<String>), Error> {
    Ok(match via {
        Via::Moshcode => {
            let engine = model.ok_or_else(|| {
                Error::usage("RUNNER_MODEL_REQUIRED", "--via moshcode needs --model <engine>; list them with: moshcode oneshot --list")
            })?;
            ("moshcode", vec!["oneshot".into(), engine.into(), "-".into()])
        }
        Via::Infernet | Via::C0mpute => {
            let mut args: Vec<String> = if via == Via::C0mpute { vec!["infernet".into(), "chat".into()] } else { vec!["chat".into()] };
            if let Some(m) = model {
                args.extend(["--model".into(), m.into()]);
            }
            (if via == Via::C0mpute { "c0mpute" } else { "infernet" }, args)
        }
    })
}

/// Run the rendered instructions through `via`; the answer is the runner's stdout.
pub fn run(via: Via, model: Option<&str>, instructions: &str) -> Result<Value, Error> {
    let (prog, args) = command(via, model)?;
    let path = link::which(prog).ok_or_else(|| Error::preflight("RUNNER_MISSING", format!("{prog} is not installed. Install it with: w3bs install {}", via.package())))?;
    let mut child = Command::new(&path)
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| Error::preflight("RUNNER_SPAWN", format!("{prog}: {e}")))?;
    // A runner that exits early closes its stdin; its exit status says why.
    let _ = child.stdin.take().expect("piped stdin").write_all(instructions.as_bytes());
    let out = child.wait_with_output().map_err(|e| Error::package("RUNNER_FAILED", format!("{prog}: {e}")))?;
    if !out.status.success() {
        return Err(Error::package("RUNNER_FAILED", format!("{prog} {} exited with {}", args.join(" "), out.status)));
    }
    Ok(json!({
        "via": format!("{via:?}").to_lowercase(),
        "command": std::iter::once(prog.to_owned()).chain(args).collect::<Vec<_>>(),
        "model": model,
        "answer": String::from_utf8_lossy(&out.stdout).trim_end(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runner_commands() {
        assert_eq!(command(Via::Moshcode, Some("claude")).unwrap(), ("moshcode", vec!["oneshot".into(), "claude".into(), "-".into()]));
        assert_eq!(command(Via::Moshcode, None).unwrap_err().info().code, "RUNNER_MODEL_REQUIRED");
        assert_eq!(command(Via::Infernet, None).unwrap(), ("infernet", vec!["chat".into()]));
        assert_eq!(command(Via::Infernet, Some("qwen2.5:7b")).unwrap().1, vec!["chat", "--model", "qwen2.5:7b"]);
        assert_eq!(command(Via::C0mpute, Some("qwen2.5:7b")).unwrap(), ("c0mpute", vec!["infernet".into(), "chat".into(), "--model".into(), "qwen2.5:7b".into()]));
    }
}
