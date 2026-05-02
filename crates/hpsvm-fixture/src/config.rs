#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ResultConfig {
    pub panic: bool,
    pub verbose: bool,
}

impl Default for ResultConfig {
    fn default() -> Self {
        Self { panic: true, verbose: false }
    }
}
