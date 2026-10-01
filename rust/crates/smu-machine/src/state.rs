//! Machine state plumbing. Ledger row `state serializer` keeps the byte layout
//! identical to C++ `mu2000::save_state()`: "S2MU" magic + state_version + ordered
//! per-device field dumps. Boot-cache envelope ("S2BC" v1) lives in `bootcache`
//! (added with the M5 row).

/// Placeholder until the M5 row lands: bytes are carried verbatim.
pub struct StateBlob(pub Vec<u8>);
