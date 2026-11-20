//! Unreal Engine source normalisation for the C++ parser.
//!
//! UE code is full of macros that tree-sitter can't expand: `class ARENA_API
//! AFoo`, `UCLASS(...)`, `UFUNCTION(...)`, `GENERATED_BODY()`. Left in place
//! they derail parsing (`ARENA_API` is read as the class name). We blank them
//! with spaces, keeping every byte offset and newline where it was so line
//! numbers stay exact, and keep the reflection specifiers as attributes on
//! the declaration that follows (they decide RPC / Blueprint semantics).

use std::collections::HashMap;

pub struct Normalized {
    pub src: Vec<u8>,
    /// Line of the declaration a reflection macro annotates → its specifiers,
    /// e.g. `["UFUNCTION", "Server", "Reliable"]`.
    pub attrs: HashMap<u32, Vec<String>>,
    /// Types introduced by `DECLARE_*DELEGATE*` / `DECLARE_EVENT*` macros: (name, line).
    pub delegates: Vec<(String, u32)>,
}

/// Reflection macros whose specifiers we keep.
const REFLECTION: &[&str] = &["UCLASS", "USTRUCT", "UENUM", "UINTERFACE", "UFUNCTION", "UPROPERTY", "UDELEGATE"];
/// Macros removed together with their argument list.
const STRIP_CALL: &[&str] = &[
    "UPARAM", "UMETA", "GENERATED_BODY", "GENERATED_UCLASS_BODY", "GENERATED_USTRUCT_BODY", "GENERATED_IINTERFACE_BODY",
    "GENERATED_UINTERFACE_BODY", "PURE_VIRTUAL", "UE_DEPRECATED", "DEPRECATED", "UE_DEPRECATED_FORGAME",
];
/// Bare macros removed on their own.
const STRIP_WORD: &[&str] = &["FORCEINLINE", "FORCENOINLINE", "FORCEINLINE_DEBUGGABLE", "UE_NODISCARD", "UE_INLINE"];

/// Specifiers that mean "the body lives in `<Name>_Implementation`".
pub const IMPLEMENTATION_SPECIFIERS: &[&str] = &["Server", "Client", "NetMulticast", "BlueprintNativeEvent"];

pub fn looks_like_unreal(src: &[u8]) -> bool {
    let s = String::from_utf8_lossy(&src[..src.len().min(64 * 1024)]);
    s.contains("GENERATED_") || s.contains("UCLASS") || s.contains("UFUNCTION") || s.contains("UPROPERTY")
        || s.contains("USTRUCT") || s.contains("_API ") || s.contains("DECLARE_")
}

pub fn normalize(src: &[u8]) -> Normalized {
    let mut out = src.to_vec();
    let mut attrs = HashMap::new();
    let mut delegates = vec![];
    let n = src.len();
    let mut i = 0;
    let mut line: u32 = 1;
    while i < n {
        let c = src[i];
        match c {
            b'\n' => {
                line += 1;
                i += 1;
            }
            b'/' if i + 1 < n && src[i + 1] == b'/' => {
                while i < n && src[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if i + 1 < n && src[i + 1] == b'*' => {
                i += 2;
                while i + 1 < n && !(src[i] == b'*' && src[i + 1] == b'/') {
                    if src[i] == b'\n' {
                        line += 1;
                    }
                    i += 1;
                }
                i += 2;
            }
            b'"' | b'\'' => i = skip_literal(src, i, &mut line),
            c if c == b'_' || c.is_ascii_alphabetic() => {
                let start = i;
                while i < n && (src[i] == b'_' || src[i].is_ascii_alphanumeric()) {
                    i += 1;
                }
                // Skip the tail of an identifier we jumped into mid-way (e.g. after a digit).
                if start > 0 && (src[start - 1] == b'_' || src[start - 1].is_ascii_alphanumeric()) {
                    continue;
                }
                let word = std::str::from_utf8(&src[start..i]).unwrap_or("");
                let is_api = word.len() > 4 && word.ends_with("_API") && word.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_');
                if is_api || STRIP_WORD.contains(&word) {
                    blank(&mut out, start, i);
                    continue;
                }
                let reflection = REFLECTION.contains(&word);
                let strip = STRIP_CALL.contains(&word);
                let declare = word.starts_with("DECLARE_") && (word.contains("DELEGATE") || word.contains("EVENT"));
                // Other file-scope registration macros (`DEFINE_LOG_CATEGORY(...)`, `IMPLEMENT_PRIMARY_GAME_MODULE(...)`).
                let boilerplate = !declare
                    && (word.starts_with("DECLARE_") || word.starts_with("DEFINE_") || word.starts_with("IMPLEMENT_"))
                    && word.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_');
                if !(reflection || strip || declare || boilerplate) {
                    continue;
                }
                let mut j = i;
                while j < n && (src[j] == b' ' || src[j] == b'\t') {
                    j += 1;
                }
                if j >= n || src[j] != b'(' {
                    continue;
                }
                let start_line = line;
                let Some(close) = matching_paren(src, j, &mut line) else { continue };
                let args = String::from_utf8_lossy(&src[j + 1..close]).to_string();
                if reflection {
                    let mut k = close + 1;
                    let mut attach = line;
                    while k < n && src[k].is_ascii_whitespace() {
                        if src[k] == b'\n' {
                            attach += 1;
                        }
                        k += 1;
                    }
                    let mut spec = vec![word.to_string()];
                    spec.extend(split_specifiers(&args));
                    attrs.insert(attach, spec);
                }
                if declare {
                    if let Some(name) = args.split(',').next().map(|s| s.trim()) {
                        if !name.is_empty() && name.bytes().all(|b| b == b'_' || b.is_ascii_alphanumeric()) {
                            delegates.push((name.to_string(), start_line));
                        }
                    }
                }
                let mut end = close + 1;
                if (declare || boilerplate) && end < n && src[end] == b';' {
                    end += 1;
                }
                blank(&mut out, start, end);
                i = close + 1;
            }
            _ => i += 1,
        }
    }
    Normalized { src: out, attrs, delegates }
}

fn blank(out: &mut [u8], from: usize, to: usize) {
    for b in &mut out[from..to] {
        if *b != b'\n' {
            *b = b' ';
        }
    }
}

fn skip_literal(src: &[u8], mut i: usize, line: &mut u32) -> usize {
    let quote = src[i];
    i += 1;
    while i < src.len() && src[i] != quote {
        if src[i] == b'\\' {
            i += 1;
        } else if src[i] == b'\n' {
            *line += 1;
        }
        i += 1;
    }
    i + 1
}

fn matching_paren(src: &[u8], open: usize, line: &mut u32) -> Option<usize> {
    let mut depth = 0;
    let mut i = open;
    let mut l = *line;
    while i < src.len() {
        match src[i] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    *line = l;
                    return Some(i);
                }
            }
            b'\n' => l += 1,
            b'"' | b'\'' => {
                i = skip_literal(src, i, &mut l);
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// `Server, Reliable, meta=(DisplayName="x"), ReplicatedUsing=OnRep_Health` →
/// `["Server", "Reliable", "meta", "ReplicatedUsing=OnRep_Health"]`
fn split_specifiers(args: &str) -> Vec<String> {
    let mut out = vec![];
    let mut depth = 0;
    let mut cur = String::new();
    for ch in args.chars() {
        match ch {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                push_spec(&mut out, &cur);
                cur.clear();
            }
            _ if depth == 0 => cur.push(ch),
            _ => {}
        }
    }
    push_spec(&mut out, &cur);
    out
}

fn push_spec(out: &mut Vec<String>, raw: &str) {
    let s: String = raw.split_whitespace().collect::<Vec<_>>().join("");
    let s = s.trim_end_matches('=').to_string();
    if !s.is_empty() {
        out.push(s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blanks_macros_and_keeps_lines() {
        let src = b"UCLASS(Blueprintable)\nclass ARENA_API AFoo : public AActor\n{\n\tGENERATED_BODY()\n\tUFUNCTION(Server, Reliable, meta=(X=\"a,b\"))\n\tvoid ServerFire();\n};\nDECLARE_DYNAMIC_MULTICAST_DELEGATE_OneParam(FOnHit, float, Damage);\n";
        let n = normalize(src);
        assert_eq!(n.src.len(), src.len());
        let text = String::from_utf8(n.src).unwrap();
        assert_eq!(text.lines().count(), 8);
        assert!(text.contains("class           AFoo"));
        assert!(!text.contains("GENERATED_BODY"));
        assert_eq!(n.attrs[&2], vec!["UCLASS", "Blueprintable"]);
        assert_eq!(n.attrs[&6], vec!["UFUNCTION", "Server", "Reliable", "meta"]);
        assert_eq!(n.delegates, vec![("FOnHit".to_string(), 8)]);
    }
}
