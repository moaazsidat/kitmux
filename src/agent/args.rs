//! Keeping an agent's flags while dropping what must not be replayed.
//!
//! A resumed agent should start with the same model, permission mode, and so
//! on, but not with the original prompt (it would be sent again) or the
//! original resume flags (they would pick a different session). Telling a
//! flag's value from a positional prompt needs to know which flags take values,
//! so each adapter supplies that from its agent's `--help`.

/// What an adapter knows about its agent's flags.
pub struct Flags<'a> {
    /// Flags that take exactly one value.
    pub value: &'a [&'a str],
    /// Flags that take every value up to the next flag.
    pub variadic: &'a [&'a str],
    /// Flags whose value is optional: a following non-flag is taken as theirs.
    pub optional: &'a [&'a str],
    /// Flags to remove, with their values.
    pub drop: &'a [&'a str],
}

/// The flags in `args` worth replaying, with their values. Positional
/// arguments, `--` and everything after it, and `drop`ped flags are removed.
pub fn keep(args: &[String], flags: &Flags) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        i += 1;
        if arg == "--" {
            break;
        }
        if !arg.starts_with('-') || arg == "-" {
            continue;
        }
        let (name, inline) = match arg.split_once('=') {
            Some((n, _)) => (n, true),
            None => (arg.as_str(), false),
        };
        let mut taken = vec![arg.clone()];
        if !inline {
            let n = if flags.value.contains(&name) {
                usize::from(i < args.len())
            } else if flags.optional.contains(&name) {
                usize::from(i < args.len() && !args[i].starts_with('-'))
            } else if flags.variadic.contains(&name) {
                args[i..].iter().take_while(|a| !a.starts_with('-')).count()
            } else {
                0
            };
            taken.extend(args[i..i + n].iter().cloned());
            i += n;
        }
        if !flags.drop.contains(&name) {
            out.extend(taken);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const FLAGS: Flags = Flags {
        value: &["--model", "-n"],
        variadic: &["--add-dir"],
        optional: &["--resume", "-r"],
        drop: &["--resume", "-r", "--continue", "-c"],
    };

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn keeps_flags_and_their_values() {
        let args = v(&[
            "--model",
            "big",
            "--yolo",
            "-n",
            "name",
            "--add-dir",
            "a",
            "b",
            "--verbose",
        ]);
        assert_eq!(keep(&args, &FLAGS), args);
    }

    #[test]
    fn drops_prompts_and_resume_flags() {
        let args = v(&["--resume", "abc", "--model=big", "fix the bug", "-c"]);
        assert_eq!(keep(&args, &FLAGS), v(&["--model=big"]));

        // An optional value is only taken when it is not another flag.
        let args = v(&["--resume", "--model", "big"]);
        assert_eq!(keep(&args, &FLAGS), v(&["--model", "big"]));

        let args = v(&["--yolo", "--", "--model", "x"]);
        assert_eq!(keep(&args, &FLAGS), v(&["--yolo"]));
    }

    #[test]
    fn a_value_flag_at_the_end_keeps_nothing_extra() {
        assert_eq!(keep(&v(&["--model"]), &FLAGS), v(&["--model"]));
    }
}
