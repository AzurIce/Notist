pub(crate) const OFFICIAL_DOCS_ARCHIVE: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/official-docs.bundle.gz"));
pub(crate) const OFFICIAL_DOCS_FINGERPRINT: &str = env!("NOTIST_DOCS_FINGERPRINT");
pub(crate) const NOTIST_SKILL_MD: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/notist-skill.md"));
pub(crate) const NOTIST_SKILL_FINGERPRINT: &str = env!("NOTIST_SKILL_FINGERPRINT");
