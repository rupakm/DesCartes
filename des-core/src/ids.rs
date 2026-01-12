use uuid::Uuid;

fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Deterministically derive a UUID from a seed, domain, and counter.
///
/// This is used to keep component and task IDs reproducible across runs.
pub fn deterministic_uuid(seed: u64, domain: u64, counter: u64) -> Uuid {
    // Mix the seed, domain, and counter into two 64-bit words.
    let x0 = seed ^ domain ^ counter;
    let lo = splitmix64(x0);
    let hi = splitmix64(x0.wrapping_add(0xD1B5_4A32_D192_ED03));
    Uuid::from_u128(((hi as u128) << 64) | (lo as u128))
}

pub const UUID_DOMAIN_COMPONENT: u64 = 0x434F_4D50_4F4E_454E; // "COMPONEN" (tag)
pub const UUID_DOMAIN_TASK: u64 = 0x5441_534B_5F49_445F; // "TASK_ID_" (tag)

// Domains for IDs created outside the normal Simulation/Scheduler paths.
// These are used to avoid non-deterministic UUID generation.
pub const UUID_DOMAIN_KEY: u64 = 0x4B45_595F_5F49_445F; // "KEY__ID_" (tag)
pub const UUID_DOMAIN_MANUAL_TASK: u64 = 0x4D41_4E55_5441_534B; // "MANUTASK" (tag)
