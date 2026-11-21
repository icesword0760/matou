use crate::flow::Utf8Decoder;
use matou_native_core::pty::LaunchProfile;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct InteractiveLaunch {
    pub profile: LaunchProfile,
    pub mode: &'static str,
}

#[derive(Clone, Default)]
pub(crate) struct ShellInput {
    line: Vec<u16>,
    decoder: Utf8Decoder,
}

impl ShellInput {
    pub(crate) fn observe(&mut self, bytes: &[u8]) -> Option<InteractiveLaunch> {
        let mut submitted = None;
        for character in self.decoder.feed(bytes).chars() {
            match character {
                '\r' | '\n' => submitted = Some(std::mem::take(&mut self.line)),
                '\x08' | '\x7f' => {
                    self.line.pop();
                }
                '\x03' | '\x15' => self.line.clear(),
                ' '.. => self
                    .line
                    .extend(character.encode_utf16(&mut [0; 2]).iter().copied()),
                _ => {}
            }
        }
        // The frozen caller forgets the buffer after any submission, including
        // the suffix after the last newline. Only the last command is matched.
        let command = submitted?;
        self.line.clear();
        String::from_utf16(&command)
            .ok()
            .as_deref()
            .and_then(interactive_launch)
    }

    pub(crate) fn clear(&mut self) {
        self.line.clear();
    }
}

pub(crate) const CODEX_BYPASS: &str = "--dangerously-bypass-approvals-and-sandbox";

fn interactive_launch(command: &str) -> Option<InteractiveLaunch> {
    if crate::dsh_launch::matches_shell_command(command) {
        return Some(InteractiveLaunch {
            profile: LaunchProfile::Dsh,
            mode: "default",
        });
    }

    if let Some(permission) = crate::antigravity_launch::interactive_permission(command) {
        return Some(InteractiveLaunch {
            profile: LaunchProfile::Antigravity,
            mode: permission.as_str(),
        });
    }

    // Match the frozen JavaScript trim and whitespace folding.
    let mut words = command
        .split(|c: char| c == '\u{feff}' || (c.is_whitespace() && c != '\u{0085}'))
        .filter(|word| !word.is_empty());
    let (profile, mode) = match (words.next(), words.next(), words.next()) {
        (Some("claude"), None, None) => (LaunchProfile::ClaudeCode, "default"),
        (Some("claude"), Some("--dangerously-skip-permissions"), None) => {
            (LaunchProfile::ClaudeCode, "bypassPermissions")
        }
        (Some("codex"), None, None) => (LaunchProfile::Codex, "default"),
        (Some("codex"), Some(CODEX_BYPASS), None) => (LaunchProfile::Codex, "bypassPermissions"),
        _ => return None,
    };
    Some(InteractiveLaunch { profile, mode })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_exact_provider_commands_are_recognized() {
        for (line, expected) in [
            ("claude", Some("default")),
            ("  claude  ", Some("default")),
            (
                "claude --dangerously-skip-permissions",
                Some("bypassPermissions"),
            ),
            (
                " \u{feff}claude   \t--dangerously-skip-permissions\u{a0}",
                Some("bypassPermissions"),
            ),
            ("claude -p", None),
            ("claude foo", None),
            ("sudo claude", None),
            ("echo claude", None),
            ("CLAUDE", None),
            ("cc", None),
        ] {
            assert_eq!(
                interactive_launch(line).map(|launch| (launch.profile, launch.mode)),
                expected.map(|mode| (LaunchProfile::ClaudeCode, mode)),
                "{line:?}"
            );
        }
    }

    #[test]
    fn codex_requires_the_exact_command_after_normalization() {
        for (command, expected) in [
            ("codex", Some("default")),
            ("  codex\u{a0}", Some("default")),
            (
                "codex --dangerously-bypass-approvals-and-sandbox",
                Some("bypassPermissions"),
            ),
            (
                " \u{feff}codex   \t--dangerously-bypass-approvals-and-sandbox ",
                Some("bypassPermissions"),
            ),
            ("codex --model fixture", None),
            (
                "codex --dangerously-bypass-approvals-and-sandbox extra",
                None,
            ),
            ("codex --dangerously-skip-permissions", None),
            ("env codex", None),
            ("echo codex", None),
            ("./codex", None),
            ("CODEX", None),
        ] {
            assert_eq!(
                interactive_launch(command),
                expected.map(|mode| InteractiveLaunch {
                    profile: LaunchProfile::Codex,
                    mode
                }),
                "{command:?}"
            );
        }
        let mut buffer = ShellInput::default();
        assert_eq!(buffer.observe(b"  co"), None);
        assert_eq!(buffer.observe(b"dex  "), None);
        assert_eq!(
            buffer.observe(b"\r"),
            Some(InteractiveLaunch {
                profile: LaunchProfile::Codex,
                mode: "default"
            })
        );
    }

    #[test]
    fn shell_input_tracks_printable_bytes_and_line_edits_across_writes() {
        for keys in [
            ["clau", "dxx\x7f\x08e", "\r"],
            ["xyz", "\x03claude", "\n"],
            ["xyz", "\x15claude", "\r"],
            ["claud你", "\x7fe", "\r"],
        ] {
            let mut buffer = ShellInput::default();
            assert!(buffer.observe(keys[0].as_bytes()).is_none());
            assert!(buffer.observe(keys[1].as_bytes()).is_none());
            assert_eq!(
                buffer.observe(keys[2].as_bytes()).map(|launch| launch.mode),
                Some("default")
            );
            assert!(buffer.observe(b"\r").is_none());
        }
        let mut buffer = ShellInput::default();
        assert!(buffer.observe(b"claude\x03\r").is_none());
        assert!(buffer.observe(b"claude\x15\n").is_none());
        assert!(buffer.observe(b"CLAUDE\r").is_none());
        assert_eq!(
            buffer
                .observe(b"  claude   --dangerously-skip-permissions  \n")
                .map(|launch| launch.mode),
            Some("bypassPermissions")
        );
    }

    #[test]
    fn only_the_last_submission_in_a_write_can_match() {
        for (bytes, expected) in [
            (&b"claude\r"[..], Some("default")),
            (&b"claude\r\n"[..], None),
            (&b"claude\rls\r"[..], None),
            (&b"touch x\rclaude\r"[..], Some("default")),
            (&b"ls\rclaude\rtail"[..], Some("default")),
        ] {
            assert_eq!(
                ShellInput::default()
                    .observe(bytes)
                    .map(|launch| launch.mode),
                expected,
                "{bytes:?}"
            );
        }
        let mut buffer = ShellInput::default();
        assert_eq!(buffer.observe(b"ls\rclaude"), None);
        assert_eq!(buffer.observe(b"\r"), None);
    }

    #[test]
    fn backspace_removes_one_utf16_code_unit() {
        for (keys, expected) in [
            ("claude😀\x7f\r", None),
            ("claude😀\x08\r", None),
            ("claude😀\x7f\x08\r", Some("default")),
            ("claude\x1b\t\x01\r", Some("default")),
        ] {
            assert_eq!(
                ShellInput::default()
                    .observe(keys.as_bytes())
                    .map(|launch| launch.mode),
                expected
            );
        }
    }

    #[test]
    fn split_chinese_character_takes_one_backspace() {
        let mut buffer = ShellInput::default();
        let (first, second) = "你".as_bytes().split_at(2);
        assert_eq!(buffer.observe(b"claude"), None);
        assert_eq!(buffer.observe(first), None);
        assert_eq!(buffer.observe(second), None);
        assert_eq!(
            buffer.observe(b"\x7f\r").map(|launch| launch.mode),
            Some("default")
        );
    }

    #[test]
    fn interrupt_clears_the_line_and_preserves_a_partial_utf8_character() {
        let mut buffer = ShellInput::default();
        let (first, second) = "你".as_bytes().split_at(2);
        assert_eq!(buffer.observe(b"previous line"), None);
        assert_eq!(buffer.observe(first), None);
        buffer.clear();
        assert_eq!(buffer.observe(second), None);
        assert_eq!(buffer.line, ['你' as u16]);
        assert_eq!(
            buffer.observe(b"\x7fclaude\r").map(|launch| launch.mode),
            Some("default")
        );
    }
}
