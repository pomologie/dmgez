use std::{path::PathBuf, sync::Arc};

use clap::Parser;
use rmcp::{
	ErrorData, Json, ServerHandler, ServiceExt,
	handler::server::{router::tool::ToolRouter, wrapper::Parameters},
	model::{Implementation, ProtocolVersion, ServerCapabilities, ServerConfig},
	schemars::{self, JsonSchema},
	tool, tool_handler, tool_router,
};
use serde::{Deserialize, Serialize};
use tokio::{runtime::Builder, sync::Semaphore};

use crate::project::{MAX_FILE_NAME_BYTES, MAX_PROJECT_BYTES, ProjectRoot, Validation};
use crate::transport::{MAX_IN_FLIGHT, MAX_REQUEST_BYTES, stdio};

const TOOL_SCHEMA_VERSION: u16 = 1;
const MAX_VALIDATIONS: usize = 1;

#[derive(Parser)]
#[command(version, about)]
pub(super) struct Options {
	/// Authorize validation of TOML files directly in this directory.
	#[arg(long, value_name = "DIRECTORY")]
	project_root: PathBuf,
}

#[derive(Debug, Clone)]
struct Server {
	root: Arc<ProjectRoot>,
	validation_slots: Arc<Semaphore>,
	tool_router: ToolRouter<Self>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CapabilitiesRequest {}

#[derive(Serialize, JsonSchema)]
struct CapabilitiesReport {
	schema_version: u16,
	manifest_schema_version: i64,
	max_manifest_bytes: usize,
	max_request_bytes: usize,
	max_in_flight: usize,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ValidationRequest {
	/// TOML file name relative to the configured project root.
	#[schemars(length(min = 1, max = MAX_FILE_NAME_BYTES))]
	file_name: String,
}

#[derive(Serialize, JsonSchema)]
struct ValidationReport {
	schema_version: u16,
	outcome: Validation,
}

#[tool_router]
impl Server {
	#[tool(annotations(read_only_hint = true, open_world_hint = false))]
	/// Return the MCP tool schema version and project validation limits.
	fn capabilities(&self, _: Parameters<CapabilitiesRequest>) -> Json<CapabilitiesReport> {
		Json(CapabilitiesReport {
			schema_version: TOOL_SCHEMA_VERSION,
			manifest_schema_version: dmgez_config::SCHEMA_VERSION,
			max_manifest_bytes: MAX_PROJECT_BYTES,
			max_request_bytes: MAX_REQUEST_BYTES,
			max_in_flight: MAX_IN_FLIGHT,
		})
	}

	#[tool(annotations(read_only_hint = true, open_world_hint = false))]
	/// Validate a project manifest in the configured directory. Only the TOML file is read.
	async fn project_validate(
		&self,
		Parameters(request): Parameters<ValidationRequest>,
	) -> Result<Json<ValidationReport>, ErrorData> {
		let outcome = match self.validation_slots.clone().try_acquire_owned() {
			Ok(permit) => {
				let root = self.root.clone();
				tokio::task::spawn_blocking(move || {
					let _permit = permit;
					root.validate(&request.file_name)
				})
				.await
				.map_err(|_| ErrorData::internal_error("validation worker failed", None))?
			}
			Err(_) => Validation::Busy,
		};
		Ok(Json(ValidationReport {
			schema_version: TOOL_SCHEMA_VERSION,
			outcome,
		}))
	}
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Server {
	fn get_info(&self) -> ServerConfig {
		ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
			.with_protocol_version(ProtocolVersion::V_2025_11_25)
			.with_server_info(Implementation::new(
				env!("CARGO_PKG_NAME"),
				env!("CARGO_PKG_VERSION"),
			))
	}
}

pub(super) fn run(options: Options) -> Result<(), Box<dyn std::error::Error>> {
	let server = Server {
		root: Arc::new(ProjectRoot::open(&options.project_root)?),
		validation_slots: Arc::new(Semaphore::new(MAX_VALIDATIONS)),
		tool_router: Server::tool_router(),
	};
	let runtime = Builder::new_current_thread().enable_time().build()?;
	runtime.block_on(async {
		server.serve(stdio()).await?.waiting().await?;
		Ok(())
	})
}
