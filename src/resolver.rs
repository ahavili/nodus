use anyhow::{Result, bail};

pub fn sync(_locked: bool, _allow_high_sensitivity: bool) -> Result<()> {
    bail!("sync is not implemented yet")
}

pub fn doctor() -> Result<()> {
    bail!("doctor is not implemented yet")
}
