pub const VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (commit ",
    env!("HIMITSU_GIT_SHA"),
    ")"
);
pub const VERSION_LINE: &str = concat!(
    "himitsu ",
    env!("CARGO_PKG_VERSION"),
    " (commit ",
    env!("HIMITSU_GIT_SHA"),
    ")"
);
