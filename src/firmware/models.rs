use semver::Version;

#[derive(Clone, Debug)]
pub struct FirmwareInfo {
    pub binary: Vec<u8>,
    pub crc: u32,
    pub version: Version,
    pub size: usize,
}
