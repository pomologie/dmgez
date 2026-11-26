// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::cmp::Ordering;

/// Denotes type of code requirements.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[repr(u32)]
pub enum RequirementType {
    /// What hosts may run on us.
    Host,
    /// What guests we may run.
    Guest,
    /// Designated requirement.
    Designated,
    /// What libraries we may link against.
    Library,
    /// What plug-ins we may load.
    Plugin,
    /// Unknown requirement type.
    Unknown(u32),
}

impl From<u32> for RequirementType {
    fn from(v: u32) -> Self {
        match v {
            1 => Self::Host,
            2 => Self::Guest,
            3 => Self::Designated,
            4 => Self::Library,
            5 => Self::Plugin,
            _ => Self::Unknown(v),
        }
    }
}

impl From<RequirementType> for u32 {
    fn from(t: RequirementType) -> Self {
        match t {
            RequirementType::Host => 1,
            RequirementType::Guest => 2,
            RequirementType::Designated => 3,
            RequirementType::Library => 4,
            RequirementType::Plugin => 5,
            RequirementType::Unknown(v) => v,
        }
    }
}

impl PartialOrd for RequirementType {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for RequirementType {
    fn cmp(&self, other: &Self) -> Ordering {
        u32::from(*self).cmp(&u32::from(*other))
    }
}

impl std::fmt::Display for RequirementType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Host => f.write_str("host(1)"),
            Self::Guest => f.write_str("guest(2)"),
            Self::Designated => f.write_str("designated(3)"),
            Self::Library => f.write_str("library(4)"),
            Self::Plugin => f.write_str("plugin(5)"),
            Self::Unknown(v) => f.write_fmt(format_args!("unknown({v})")),
        }
    }
}
