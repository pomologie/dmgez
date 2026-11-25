// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Common cryptography primitives.

use {
    crate::AppleCodesignError,
    std::{
        borrow::Cow,
        cmp::Ordering,
        fmt::{Display, Formatter},
        path::Path,
    },
};

/// Represents a digest type encountered in code signature data structures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DigestType {
    None,
    Sha1,
    Sha256,
    Sha256Truncated,
    Sha384,
    Sha512,
    Unknown(u8),
}

impl Default for DigestType {
    fn default() -> Self {
        Self::Sha256
    }
}

impl PartialOrd for DigestType {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for DigestType {
    fn cmp(&self, other: &Self) -> Ordering {
        u8::from(*self).cmp(&u8::from(*other))
    }
}

impl Display for DigestType {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            DigestType::None => f.write_str("none"),
            DigestType::Sha1 => f.write_str("sha1"),
            DigestType::Sha256 => f.write_str("sha256"),
            DigestType::Sha256Truncated => f.write_str("sha256-truncated"),
            DigestType::Sha384 => f.write_str("sha384"),
            DigestType::Sha512 => f.write_str("sha512"),
            DigestType::Unknown(v) => f.write_fmt(format_args!("unknown: {v}")),
        }
    }
}

impl TryFrom<&str> for DigestType {
    type Error = AppleCodesignError;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        match s {
            "none" => Ok(Self::None),
            "sha1" => Ok(Self::Sha1),
            "sha256" => Ok(Self::Sha256),
            "sha256-truncated" => Ok(Self::Sha256Truncated),
            "sha384" => Ok(Self::Sha384),
            "sha512" => Ok(Self::Sha512),
            _ => Err(AppleCodesignError::DigestUnknownAlgorithm),
        }
    }
}

impl DigestType {
    /// Obtain the size of hashes for this hash type.
    pub fn hash_len(&self) -> Result<usize, AppleCodesignError> {
        Ok(self.digest_data(&[])?.len())
    }

    /// Obtain a hasher for this digest type.
    pub fn as_hasher(&self) -> Result<ring::digest::Context, AppleCodesignError> {
        match self {
            Self::None => Err(AppleCodesignError::DigestUnknownAlgorithm),
            Self::Sha1 => Ok(ring::digest::Context::new(
                &ring::digest::SHA1_FOR_LEGACY_USE_ONLY,
            )),
            Self::Sha256 | Self::Sha256Truncated => {
                Ok(ring::digest::Context::new(&ring::digest::SHA256))
            }
            Self::Sha384 => Ok(ring::digest::Context::new(&ring::digest::SHA384)),
            Self::Sha512 => Ok(ring::digest::Context::new(&ring::digest::SHA512)),
            Self::Unknown(_) => Err(AppleCodesignError::DigestUnknownAlgorithm),
        }
    }

    /// Digest data given the configured hasher.
    pub fn digest_data(&self, data: &[u8]) -> Result<Vec<u8>, AppleCodesignError> {
        let mut hasher = self.as_hasher()?;

        hasher.update(data);
        let mut hash = hasher.finish().as_ref().to_vec();

        if matches!(self, Self::Sha256Truncated) {
            hash.truncate(20);
        }

        Ok(hash)
    }
}

pub struct Digest<'a> {
    pub data: Cow<'a, [u8]>,
}

impl<'a> Digest<'a> {
    /// Whether this is the null hash (all 0s).
    pub fn is_null(&self) -> bool {
        self.data.iter().all(|b| *b == 0)
    }

    pub fn to_vec(&self) -> Vec<u8> {
        self.data.to_vec()
    }

    pub fn to_owned(&self) -> Digest<'static> {
        Digest {
            data: Cow::Owned(self.data.clone().into_owned()),
        }
    }

    pub fn as_hex(&self) -> String {
        hex::encode(&self.data)
    }
}

impl<'a> std::fmt::Debug for Digest<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&hex::encode(&self.data))
    }
}

impl<'a> From<Vec<u8>> for Digest<'a> {
    fn from(v: Vec<u8>) -> Self {
        Self { data: v.into() }
    }
}

/// Holds multiple computed digests for content.
pub struct MultiDigest {
    pub sha1: Digest<'static>,
    pub sha256: Digest<'static>,
}

impl MultiDigest {
    /// Compute the multi digests for any stream reader.
    ///
    /// This will read the stream until EOF.
    pub fn from_reader(mut reader: impl std::io::Read) -> Result<Self, AppleCodesignError> {
        let mut sha1 = DigestType::Sha1.as_hasher()?;
        let mut sha256 = DigestType::Sha256.as_hasher()?;

        let mut buffer = [0u8; 16384];

        loop {
            let read = reader.read(&mut buffer)?;
            if read == 0 {
                break;
            }

            sha1.update(&buffer[0..read]);
            sha256.update(&buffer[0..read]);
        }

        let sha1 = sha1.finish().as_ref().to_vec();
        let sha256 = sha256.finish().as_ref().to_vec();

        Ok(Self {
            sha1: sha1.into(),
            sha256: sha256.into(),
        })
    }

    /// Compute the multi digest of a filesystem path.
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, AppleCodesignError> {
        let fh = std::fs::File::open(path.as_ref())?;
        Self::from_reader(fh)
    }
}
