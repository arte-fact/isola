use crate::error::IsolaError;

pub fn run() -> Result<(), IsolaError> {
    crate::backend::setup_host()
}
