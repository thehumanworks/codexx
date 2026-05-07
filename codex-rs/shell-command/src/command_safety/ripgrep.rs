use std::borrow::Cow;

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum RipgrepArgCase {
    Sensitive,
    AsciiInsensitive,
}

pub(crate) fn is_safe_ripgrep_command(command: &[String], arg_case: RipgrepArgCase) -> bool {
    !command
        .iter()
        .skip(1)
        .map(String::as_str)
        .any(|arg| is_unsafe_ripgrep_arg(arg, arg_case))
}

pub(crate) fn ripgrep_command_can_execute_arbitrary_command(
    command: &[String],
    arg_case: RipgrepArgCase,
) -> bool {
    command.iter().skip(1).map(String::as_str).any(|arg| {
        let normalized = normalize_arg(arg, arg_case);
        ripgrep_arg_can_execute_arbitrary_command(normalized.as_ref())
    })
}

fn is_unsafe_ripgrep_arg(arg: &str, arg_case: RipgrepArgCase) -> bool {
    let normalized = normalize_arg(arg, arg_case);
    if ripgrep_arg_can_execute_arbitrary_command(normalized.as_ref()) {
        return true;
    }

    match normalized.as_ref() {
        // Calls out to other decompression tools, so do not auto-approve
        // out of an abundance of caution.
        "--search-zip" => true,
        "-z" => true,
        _ => false,
    }
}

fn ripgrep_arg_can_execute_arbitrary_command(normalized_arg: &str) -> bool {
    matches!(
        normalized_arg,
        // Takes an arbitrary command that is executed for each match.
        "--pre"
        // Takes a command that can be used to obtain the local hostname.
        | "--hostname-bin"
    ) || normalized_arg.starts_with("--pre=")
        || normalized_arg.starts_with("--hostname-bin=")
}

fn normalize_arg(arg: &str, arg_case: RipgrepArgCase) -> Cow<'_, str> {
    match arg_case {
        RipgrepArgCase::Sensitive => Cow::Borrowed(arg),
        RipgrepArgCase::AsciiInsensitive => Cow::Owned(arg.to_ascii_lowercase()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vec_str(args: &[&str]) -> Vec<String> {
        args.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn rejects_ripgrep_options_that_can_spawn_processes() {
        for args in [
            vec_str(&["rg", "--pre", "pwned", "files"]),
            vec_str(&["rg", "--pre=pwned", "files"]),
            vec_str(&["rg", "--hostname-bin", "pwned", "files"]),
            vec_str(&["rg", "--hostname-bin=pwned", "files"]),
            vec_str(&["rg", "--search-zip", "files"]),
            vec_str(&["rg", "-z", "files"]),
        ] {
            assert!(
                !is_safe_ripgrep_command(&args, RipgrepArgCase::Sensitive),
                "expected {args:?} to be unsafe",
            );
        }
    }

    #[test]
    fn rejects_case_insensitive_ripgrep_options() {
        for args in [
            vec_str(&["rg", "--PRE", "pwned", "files"]),
            vec_str(&["rg", "--HOSTNAME-BIN=pwned", "files"]),
            vec_str(&["rg", "--SEARCH-ZIP", "files"]),
            vec_str(&["rg", "-Z", "files"]),
        ] {
            assert!(
                !is_safe_ripgrep_command(&args, RipgrepArgCase::AsciiInsensitive),
                "expected {args:?} to be unsafe with case insensitive matching",
            );
        }
    }
}
