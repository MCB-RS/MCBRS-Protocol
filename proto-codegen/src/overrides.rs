//! The few places where Mojang's schema under-describes the wire format.
//!
//! Everything else is generated from the schemas alone. Each entry here names
//! one property, the hand-written `crate::codec` type that replaces its
//! generated type, and why. The generator fails if an entry stops matching
//! the schemas, so these special cases cannot silently go stale.

pub struct FieldOverride {
    /// Title of the document that owns the property.
    pub document: &'static str,
    /// Property name, as written in the schema.
    pub property: &'static str,
    /// Type of `crate::codec` used for the field.
    pub codec_type: &'static str,
    /// Why the schema alone is not enough.
    #[allow(dead_code)]
    pub reason: &'static str,
}

pub const FIELD_OVERRIDES: &[FieldOverride] = &[
    FieldOverride {
        document: "LoginPacketPayload",
        property: "Connection Request",
        codec_type: "ConnectionRequest",
        reason: "typed as `string`, but the string is an envelope: u32-prefixed authentication JSON, \
                 then u32-prefixed client data JWT",
    },
    FieldOverride {
        document: "SubClientLoginPacketPayload",
        property: "Sub Client Connection Request",
        codec_type: "ConnectionRequest",
        reason: "same envelope as LoginPacketPayload",
    },
];

/// The override for `property` of the document titled `document`, if any.
pub fn find(document: &str, property: &str) -> Option<(usize, &'static FieldOverride)> {
    FIELD_OVERRIDES.iter().enumerate().find(|(_, entry)| entry.document == document && entry.property == property)
}
