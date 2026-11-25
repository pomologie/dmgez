use crate::extract::{HostName, NameProfile};

const MAX_LINK_BYTES: usize = 4_096;
const MAX_COMPONENT_BYTES: usize = 255;
const SEPARATOR: u8 = b'/';
const CURRENT: &[u8] = b".";
const PARENT: &[u8] = b"..";

pub(super) fn confined(target: &[u8], mut depth: usize, profile: NameProfile) -> bool {
	if target.is_empty() || target.len() > MAX_LINK_BYTES || target[0] == SEPARATOR {
		return false;
	}
	let mut descended = false;
	for component in target.split(|&byte| byte == SEPARATOR) {
		match component {
			b"" | CURRENT => (),
			// A preceding descent could traverse another symlink and change the
			// depth at which this parent step is evaluated by the kernel.
			PARENT if !descended && depth != 0 => depth -= 1,
			PARENT => return false,
			name => {
				let Ok(name) = std::str::from_utf8(name) else {
					return false;
				};
				if HostName::new(name, profile, MAX_COMPONENT_BYTES).is_err() {
					return false;
				}
				descended = true;
			}
		}
	}
	true
}
