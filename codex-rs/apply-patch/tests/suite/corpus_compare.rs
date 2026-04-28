use codex_apply_patch::Hunk;
use codex_apply_patch::ParseError;
use codex_apply_patch::StreamingPatchParser;
use codex_apply_patch::parse_patch;

#[derive(Debug, PartialEq)]
enum CompareResult {
    Match,
    Mismatch {
        legacy: Result<Vec<Hunk>, ParseError>,
        streaming: Result<Vec<Hunk>, ParseError>,
    },
}

fn parse_with_streaming_parser(patch: &str) -> Result<Vec<Hunk>, ParseError> {
    let mut parser = StreamingPatchParser::default();
    let mut last_hunks = None;
    if let Some(hunks) = parser.push_delta(patch)? {
        last_hunks = Some(hunks);
    }
    if !patch.ends_with('\n')
        && let Some(hunks) = parser.push_delta("\n")?
    {
        last_hunks = Some(hunks);
    }
    Ok(last_hunks.unwrap_or_default())
}

fn compare_patch_outputs(patch: &str) -> CompareResult {
    let legacy = parse_patch(patch).map(|args| args.hunks);
    let streaming = parse_with_streaming_parser(patch);
    if legacy == streaming {
        CompareResult::Match
    } else {
        CompareResult::Mismatch { legacy, streaming }
    }
}

#[test]
fn reduced_repros_document_current_parser_mismatches() {
    let cases = [
        (
            "empty update hunk is accepted by streaming parser",
            "\
*** Begin Patch
*** Update File: foo.txt
*** End Patch",
        ),
        (
            "trailing empty update chunk before end patch is accepted",
            "\
*** Begin Patch
*** Update File: foo.txt
@@
-old
+new
@@
*** End Patch",
        ),
        (
            "trimmed nested add-file header inside update content is misparsed as syntax",
            "\
*** Begin Patch
*** Update File: foo.txt
@@
-old
+new
 *** Add File: nested.txt
 +hello
 *** End Patch
*** End Patch",
        ),
        (
            "trimmed nested context marker inside update content starts a new chunk",
            "\
*** Begin Patch
*** Update File: foo.txt
@@
 line before
 @@ nested
-line after
+line after new
*** End Patch",
        ),
        (
            "trimmed nested end marker inside update content ends the patch early",
            "\
*** Begin Patch
*** Update File: foo.txt
@@
-old
+new
 *** End Patch
 tail
*** End Patch",
        ),
        (
            "move-only update hunk is accepted and next hunk continues",
            "\
*** Begin Patch
*** Update File: old.txt
*** Move to: new.txt
*** Update File: other.txt
@@
-before
+after
*** End Patch",
        ),
    ];

    for (name, patch) in cases {
        let result = compare_patch_outputs(patch);
        assert!(
            matches!(result, CompareResult::Mismatch { .. }),
            "{name}: expected mismatch, got {result:?}"
        );
    }
}
