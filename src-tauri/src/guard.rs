//! The two predicates that decide what a markdown document is allowed to reach.
//!
//! Both used to be inline in the `open_external` and `delete_file` commands.
//! `main.rs` is a `[[bin]]` and cannot be imported, so while they lived there no
//! test, example or bench could reach them — tenet 4 was enforced by code nobody
//! could assert on. The commands still own the I/O (opening, canonicalising,
//! trashing); what moved here is only the decision.

use std::path::Path;

/// Schemes a markdown document may hand to the OS opener.
///
/// Document content is untrusted, so this is an allowlist rather than a
/// denylist: `file:`, `javascript:`, `data:` and every third-party app scheme
/// are refused because they are not on it, not because anyone remembered to
/// name them. The comparison is case-folded — `JavaScript:` is the same scheme
/// as `javascript:` to the OS, and would be the obvious way past a naive check.
pub fn allowed_scheme(url: &str) -> Result<(), String> {
    let scheme = url.split_once(':').map(|(s, _)| s.to_ascii_lowercase());
    match scheme.as_deref() {
        Some("http" | "https" | "mailto") => Ok(()),
        Some(other) => Err(format!("refusing to open scheme: {other}")),
        None => Err("refusing to open URL without a scheme".into()),
    }
}

/// Is `target` the repo root itself, or somewhere beneath it?
///
/// `Path::starts_with` compares whole components, which is the entire point: a
/// textual `str::starts_with` would accept `/w/notes-private/secret.md` under a
/// root of `/w/notes`, because it never reaches a separator. Both paths must
/// already be canonical — resolving symlinks and `..` is the caller's job, and
/// doing it here would make this predicate do I/O and stop being testable.
pub fn inside_root(root: &Path, target: &Path) -> bool {
    target.starts_with(root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn allows_the_three_document_schemes() {
        for url in [
            "http://example.com",
            "https://example.com/a?b=c#d",
            "mailto:someone@example.com",
        ] {
            assert!(allowed_scheme(url).is_ok(), "should allow {url}");
        }
    }

    #[test]
    fn scheme_match_is_case_folded() {
        assert!(allowed_scheme("HTTPS://example.com").is_ok());
        assert!(allowed_scheme("MailTo:someone@example.com").is_ok());
        // ...and the fold must not become a way *in*, either.
        assert!(allowed_scheme("JavaScript:alert(1)").is_err());
    }

    #[test]
    fn refuses_everything_else() {
        for url in [
            "javascript:alert(1)",
            "file:///etc/passwd",
            "data:text/html;base64,PHNjcmlwdD4=",
            "vscode://file/etc/passwd",
            "tel:+15551234",
        ] {
            assert!(allowed_scheme(url).is_err(), "should refuse {url}");
        }
    }

    #[test]
    fn refuses_a_url_with_no_scheme() {
        let err = allowed_scheme("example.com/page").unwrap_err();
        assert!(err.contains("without a scheme"), "got {err:?}");
        // The two failures carry different messages on purpose.
        let other = allowed_scheme("ftp://example.com").unwrap_err();
        assert!(
            other.contains("refusing to open scheme: ftp"),
            "got {other:?}"
        );
    }

    #[test]
    fn root_contains_itself_and_its_descendants() {
        let root = PathBuf::from("/w/notes");
        assert!(inside_root(&root, &root));
        assert!(inside_root(&root, &PathBuf::from("/w/notes/a.md")));
        assert!(inside_root(
            &root,
            &PathBuf::from("/w/notes/deep/nest/a.md")
        ));
    }

    #[test]
    fn a_sibling_sharing_a_prefix_is_outside() {
        // The whole reason this is `Path::starts_with` and not `str`.
        let root = PathBuf::from("/w/notes");
        assert!(!inside_root(
            &root,
            &PathBuf::from("/w/notes-private/secret.md")
        ));
        assert!(!inside_root(&root, &PathBuf::from("/w/notesx")));
        assert!(!inside_root(&root, &PathBuf::from("/w/other/a.md")));
        assert!(!inside_root(&root, &PathBuf::from("/w")));
    }
}
