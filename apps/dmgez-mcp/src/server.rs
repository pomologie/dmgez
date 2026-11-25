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

use crate::project::{
	Listing, MAX_DIRECTORY_ENTRIES, MAX_FILE_NAME_BYTES, MAX_PAGE_SIZE, MAX_PROJECT_BYTES,
	ProjectRoot, Reading, Validation,
};
use crate::transport::{MAX_IN_FLIGHT, MAX_REQUEST_BYTES, stdio};

const TOOL_SCHEMA_VERSION: u16 = 1;
const MAX_PROJECT_OPERATIONS: usize = 1;

#[derive(Parser)]
#[command(version, about)]
pub(super) struct Options {
	/// Authorize read-only access to TOML project files directly in this directory.
	#[arg(long, value_name = "DIRECTORY")]
	project_root: PathBuf,
}

#[derive(Debug, Clone)]
struct Server {
	root: Arc<ProjectRoot>,
	project_slots: Arc<Semaphore>,
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
	max_directory_entries: usize,
	max_project_page_size: usize,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ProjectNameRequest {
	/// TOML file name relative to the configured project root.
	#[schemars(length(min = 1, max = MAX_FILE_NAME_BYTES))]
	file_name: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ProjectListRequest {
	/// Continue strictly after this file name from the prior page.
	#[schemars(length(min = 1, max = MAX_FILE_NAME_BYTES))]
	after: Option<String>,
	/// Return at most this many candidate project names. Defaults to the maximum.
	#[schemars(range(min = 1, max = MAX_PAGE_SIZE))]
	page_size: Option<usize>,
}

#[derive(Serialize, JsonSchema)]
struct ProjectReport<T> {
	schema_version: u16,
	outcome: T,
}

#[tool_router]
impl Server {
	#[tool(annotations(read_only_hint = true, open_world_hint = false))]
	/// Return the MCP tool schema version and project operation limits.
	fn capabilities(&self, _: Parameters<CapabilitiesRequest>) -> Json<CapabilitiesReport> {
		Json(CapabilitiesReport {
			schema_version: TOOL_SCHEMA_VERSION,
			manifest_schema_version: dmgez_config::SCHEMA_VERSION,
			max_manifest_bytes: MAX_PROJECT_BYTES,
			max_request_bytes: MAX_REQUEST_BYTES,
			max_in_flight: MAX_IN_FLIGHT,
			max_directory_entries: MAX_DIRECTORY_ENTRIES,
			max_project_page_size: MAX_PAGE_SIZE,
		})
	}

	#[tool(annotations(read_only_hint = true, open_world_hint = false))]
	/// Validate a project manifest in the configured directory. Only the TOML file is read.
	async fn project_validate(
		&self,
		Parameters(request): Parameters<ProjectNameRequest>,
	) -> Result<Json<ProjectReport<Validation>>, ErrorData> {
		self.project_operation(Validation::Busy, move |root| {
			root.validate(&request.file_name)
		})
		.await
	}

	#[tool(annotations(read_only_hint = true, open_world_hint = false))]
	/// Read a validated manifest summary and the SHA-256 revision of the exact file bytes.
	async fn project_read(
		&self,
		Parameters(request): Parameters<ProjectNameRequest>,
	) -> Result<Json<ProjectReport<Reading>>, ErrorData> {
		self.project_operation(Reading::Busy, move |root| root.read(&request.file_name))
			.await
	}

	#[tool(annotations(read_only_hint = true, open_world_hint = false))]
	/// List candidate manifest names directly in the configured directory, in byte order.
	async fn project_list(
		&self,
		Parameters(request): Parameters<ProjectListRequest>,
	) -> Result<Json<ProjectReport<Listing>>, ErrorData> {
		self.project_operation(Listing::Busy, move |root| {
			root.list(request.after.as_deref(), request.page_size)
		})
		.await
	}
}

impl Server {
	async fn project_operation<T: Send + 'static>(
		&self,
		busy: T,
		operation: impl FnOnce(&ProjectRoot) -> T + Send + 'static,
	) -> Result<Json<ProjectReport<T>>, ErrorData> {
		let outcome = match self.project_slots.clone().try_acquire_owned() {
			Ok(permit) => {
				let root = self.root.clone();
				tokio::task::spawn_blocking(move || {
					let _permit = permit;
					operation(&root)
				})
				.await
				.map_err(|_| ErrorData::internal_error("project worker failed", None))?
			}
			Err(_) => busy,
		};
		Ok(Json(ProjectReport {
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
		project_slots: Arc::new(Semaphore::new(MAX_PROJECT_OPERATIONS)),
		tool_router: Server::tool_router(),
	};
	let runtime = Builder::new_current_thread().enable_time().build()?;
	runtime.block_on(async {
		server.serve(stdio()).await?.waiting().await?;
		Ok(())
	})
}
