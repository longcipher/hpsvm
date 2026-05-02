use crate::{Fixture, FixtureError};

pub(crate) fn load(path: &std::path::Path) -> Result<Fixture, FixtureError> {
    let bytes = std::fs::read(path)?;
    bincode::deserialize(&bytes).map_err(FixtureError::DecodeFixture)
}

pub(crate) fn save(fixture: &Fixture, path: &std::path::Path) -> Result<(), FixtureError> {
    let bytes = bincode::serialize(fixture).map_err(FixtureError::EncodeFixture)?;
    std::fs::write(path, bytes)?;
    Ok(())
}
