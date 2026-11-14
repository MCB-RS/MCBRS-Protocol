//! Turns C++ / free-text names into valid, unique Rust identifiers.

use std::collections::HashSet;

/// Keywords that can be used as raw identifiers (`r#type`).
const RAW_KEYWORDS: &[&str] = &[
    "as", "break", "const", "continue", "else", "enum", "extern", "false", "fn", "for", "if", "impl", "in", "let",
    "loop", "match", "mod", "move", "mut", "pub", "ref", "return", "static", "struct", "trait", "true", "type",
    "unsafe", "use", "where", "while", "async", "await", "dyn", "abstract", "become", "box", "do", "final", "macro",
    "override", "priv", "typeof", "unsized", "virtual", "yield", "try", "gen", "union",
];

/// Keywords that cannot be raw identifiers: they get a trailing underscore.
const NON_RAW_KEYWORDS: &[&str] = &["self", "Self", "super", "crate", "_"];

/// Type names the generated code relies on (prelude and `use`d items); a
/// generated type must never shadow them.
pub const RESERVED_TYPE_NAMES: &[&str] = &[
    "Self", "Option", "Some", "None", "Result", "Ok", "Err", "Vec", "String", "Box", "Default", "Clone", "Copy",
    "Debug", "PartialEq", "Eq", "Hash", "From", "Into", "TryFrom", "TryInto", "Iterator", "Sized", "Send", "Sync",
    "BedrockCodec", "Buf", "BufMut", "ProtocolError", "ProtocolEnum", "MaybeUnknown", "NetworkNbt", "Packet", "Sample",
    "Gen", "BedrockString", "Validate",
];

/// Splits on everything that is not an ASCII letter or digit.
fn chunks(input: &str) -> impl Iterator<Item = &str> {
    input.split(|c: char| !c.is_ascii_alphanumeric()).filter(|chunk| !chunk.is_empty())
}

/// Splits a chunk on camelCase boundaries: `ClientNetworkVersion` ->
/// `Client`, `Network`, `Version`; `XMLHttp` -> `XML`, `Http`; `PackIDs` ->
/// `Pack`, `IDs`.
fn camel_words(chunk: &str) -> Vec<&str> {
    let chars: Vec<char> = chunk.chars().collect();
    let mut words = Vec::new();
    let mut start = 0;
    for i in 1..chars.len() {
        let (prev, cur) = (chars[i - 1], chars[i]);
        let lower_to_upper = (prev.is_ascii_lowercase() || prev.is_ascii_digit()) && cur.is_ascii_uppercase();
        // End of an acronym: "HTTPServer" splits before "Server"; a plural
        // acronym ("IDs") stays in one piece.
        let acronym_end = prev.is_ascii_uppercase()
            && cur.is_ascii_uppercase()
            && chars.get(i + 1).is_some_and(|next| next.is_ascii_lowercase())
            && !(chars[i + 1] == 's' && i + 2 == chars.len());
        if lower_to_upper || acronym_end {
            words.push(&chunk[start..i]);
            start = i;
        }
    }
    words.push(&chunk[start..]);
    words
}

fn capitalize(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
        None => String::new(),
    }
}

/// `PascalCase` for types and variants. Inner capitals are preserved
/// (`Color255RGB` stays as is) but shouting chunks are tamed
/// (`PLAYER_LOCATION` -> `PlayerLocation`).
pub fn pascal_case(input: &str) -> String {
    let mut out = String::new();
    for chunk in chunks(input) {
        let letters = chunk.chars().filter(|c| c.is_ascii_alphabetic()).count();
        let shouting = letters > 1 && !chunk.chars().any(|c| c.is_ascii_lowercase());
        if shouting {
            out.push_str(&capitalize(&chunk.to_ascii_lowercase()));
        } else {
            out.push_str(&capitalize(chunk));
        }
    }
    if out.is_empty() {
        out.push_str("Unnamed");
    }
    if out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    if NON_RAW_KEYWORDS.contains(&out.as_str()) {
        out.push('_');
    }
    out
}

/// `snake_case` for fields, with keyword escaping (`type` -> `r#type`,
/// `self` -> `self_`) and a `_` prefix for names starting with a digit.
pub fn snake_case(input: &str) -> String {
    let words: Vec<String> =
        chunks(input).flat_map(camel_words).map(|word| word.to_ascii_lowercase()).collect();
    let mut out = words.join("_");
    if out.is_empty() {
        out.push_str("unnamed");
    }
    if out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    escape_keyword(out)
}

/// `SCREAMING_SNAKE_CASE` for associated constants.
pub fn screaming_snake_case(input: &str) -> String {
    let words: Vec<String> =
        chunks(input).flat_map(camel_words).map(|word| word.to_ascii_uppercase()).collect();
    let mut out = words.join("_");
    if out.is_empty() {
        out.push_str("UNNAMED");
    }
    if out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    out
}

pub fn escape_keyword(ident: String) -> String {
    if RAW_KEYWORDS.contains(&ident.as_str()) {
        format!("r#{ident}")
    } else if NON_RAW_KEYWORDS.contains(&ident.as_str()) {
        format!("{ident}_")
    } else {
        ident
    }
}

/// Splits a PascalCase identifier into its words (used to shorten variant names).
pub fn pascal_words(ident: &str) -> Vec<&str> {
    camel_words(ident)
}

/// Hands out unique identifiers within one namespace.
#[derive(Default)]
pub struct Namer {
    used: HashSet<String>,
}

impl Namer {
    pub fn with_reserved(reserved: &[&str]) -> Self {
        Self { used: reserved.iter().map(|name| (*name).to_owned()).collect() }
    }

    /// Returns `candidate`, or `candidate2`, `candidate3`... if it is taken.
    pub fn unique(&mut self, candidate: &str) -> String {
        let mut name = candidate.to_owned();
        let mut counter = 2;
        // Identifiers are compared case-sensitively, like Rust does.
        while !self.used.insert(name.clone()) {
            name = format!("{candidate}{counter}");
            counter += 1;
        }
        name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pascal() {
        assert_eq!(pascal_case("ActorLink"), "ActorLink");
        assert_eq!(pascal_case("BookEditAction::ReplacePage"), "BookEditActionReplacePage");
        assert_eq!(
            pascal_case("SharedTypes::Identifier<SharedTypes::AssetType::CameraPreset>"),
            "SharedTypesIdentifierSharedTypesAssetTypeCameraPreset"
        );
        assert_eq!(pascal_case("(anonymous namespace)::Foo::Bar"), "AnonymousNamespaceFooBar");
        assert_eq!(pascal_case("Aim-Assist Target Mode"), "AimAssistTargetMode");
        assert_eq!(pascal_case("hashed_string"), "HashedString");
        assert_eq!(pascal_case("Color255RGB"), "Color255RGB");
        assert_eq!(pascal_case("PLAYER_LOCATION_COORDINATES"), "PlayerLocationCoordinates");
        assert_eq!(pascal_case("item.use.on"), "ItemUseOn");
        assert_eq!(pascal_case("3D"), "_3D");
        assert_eq!(pascal_case("self"), "Self_");
        assert_eq!(pascal_case("??"), "Unnamed");
    }

    #[test]
    fn snake() {
        assert_eq!(snake_case("Target A"), "target_a");
        assert_eq!(snake_case("ClientNetworkVersion"), "client_network_version");
        assert_eq!(snake_case("Is cache supported?"), "is_cache_supported");
        assert_eq!(snake_case("__[[CTD]]__"), "ctd");
        assert_eq!(snake_case("Structure's NBT"), "structure_s_nbt");
        assert_eq!(snake_case("XUID"), "xuid");
        assert_eq!(snake_case("PackIDs"), "pack_ids");
        assert_eq!(snake_case("HTTPServer"), "http_server");
        assert_eq!(snake_case("Vec3"), "vec3");
        assert_eq!(snake_case("0th"), "_0th");
        assert_eq!(snake_case("!!"), "unnamed");
    }

    #[test]
    fn keywords_are_escaped() {
        for keyword in ["type", "move", "in", "box", "ref", "final", "match", "loop", "use", "mod", "struct", "enum"] {
            assert_eq!(snake_case(keyword), format!("r#{keyword}"));
        }
        assert_eq!(snake_case("Type"), "r#type");
        assert_eq!(snake_case("self"), "self_");
        assert_eq!(snake_case("Super"), "super_");
        assert_eq!(snake_case("crate"), "crate_");
        // Every escaped keyword must be accepted by the Rust parser.
        for keyword in RAW_KEYWORDS {
            let ident = snake_case(keyword);
            syn::parse_str::<syn::Ident>(&ident).unwrap_or_else(|e| panic!("{ident}: {e}"));
        }
    }

    #[test]
    fn screaming() {
        assert_eq!(screaming_snake_case("Descriptor Type"), "DESCRIPTOR_TYPE");
        assert_eq!(screaming_snake_case("responseType"), "RESPONSE_TYPE");
    }

    #[test]
    fn namer_dedupes() {
        let mut namer = Namer::with_reserved(&["String"]);
        assert_eq!(namer.unique("String"), "String2");
        assert_eq!(namer.unique("Foo"), "Foo");
        assert_eq!(namer.unique("Foo"), "Foo2");
        assert_eq!(namer.unique("Foo"), "Foo3");
    }
}
