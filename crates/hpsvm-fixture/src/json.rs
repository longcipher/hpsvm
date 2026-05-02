use crate::{Fixture, FixtureError};

pub(crate) fn load(path: &std::path::Path) -> Result<Fixture, FixtureError> {
    if path.extension().and_then(|value| value.to_str()) != Some("json") {
        return Err(FixtureError::UnsupportedFormat { path: path.display().to_string() });
    }

    let file = std::fs::read_to_string(path)?;
    Ok(serde_json::from_str(&file)?)
}

pub(crate) fn save(fixture: &Fixture, path: &std::path::Path) -> Result<(), FixtureError> {
    let json = serde_json::to_string_pretty(fixture)?;
    std::fs::write(path, json)?;
    Ok(())
}
