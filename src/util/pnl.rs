pub const LAMPORTS_PER_SOL: u64 = 1_000_000_000;

pub fn lamports_to_sol(lamports: u64) -> f64 {
    (lamports as f64) / (LAMPORTS_PER_SOL as f64)
}

pub fn lamports_to_sol_signed(lamports: i128) -> f64 {
    (lamports as f64) / (LAMPORTS_PER_SOL as f64)
}
