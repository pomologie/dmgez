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

use crate::image::{self, MAX_METADATA_BYTES};
use crate::local::{LocalRoot, MAX_FILE_NAME_BYTES};
use crate::project::{
	Listing, MAX_DIRECTORY_ENTRIES, MAX_PAGE_SIZE, MAX_PROJECT_BYTES, Reading, Validation,
};
use crate::transport::{MAX_IN_FLIGHT, MAX_REQUEST_BYTES, stdio};

const TOOL_SCHEMA_VERSION: u16 = 1;
const MAX_LOCAL_OPERATIONS: usize = 1;
const IMAGE_INSPECT: &str = "image_inspect";
const IMAGE_VERIFY: &str = "image_verify";

#[derive(Parser)]
#[command(version, about)]
pub(super) struct Options {
	/// Authorize read-only access to TOML project files directly in this directory.
	#[arg(long, value_name = "DIRECTORY")]
	project_root: PathBuf,
	/// Authorize bounded UDIF metadata reads directly in this directory.
	#[arg(long, value_name = "DIRECTORY")]
	image_root: Option<PathBuf>,
}

#[derive(Debug, Clone)]
struct Server {
	project_root: Arc<LocalRoot>,
	image_root: Option<Arc<LocalRoot>>,
	local_slots: Arc<Semaphore>,
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
	image_inspection: bool,
	max_image_metadata_bytes: usize,
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

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ImageRequest {
	/// Direct `.dmg` file name relative to the configured image root.
	#[schemars(length(min = 1, max = MAX_FILE_NAME_BYTES))]
	file_name: String,
}

#[derive(Serialize, JsonSchema)]
struct ToolReport<T> {
	schema_version: u16,
	outcome: T,
}

#[tool_router]
impl Server {
	#[tool(annotations(read_only_hint = true, open_world_hint = false))]
	/// Return the MCP tool schema version, local operation limits, and available image tools.
	fn capabilities(&self, _: Parameters<CapabilitiesRequest>) -> Json<CapabilitiesReport> {
		Json(CapabilitiesReport {
			schema_version: TOOL_SCHEMA_VERSION,
			manifest_schema_version: dmgez_config::SCHEMA_VERSION,
			max_manifest_bytes: MAX_PROJECT_BYTES,
			max_request_bytes: MAX_REQUEST_BYTES,
			max_in_flight: MAX_IN_FLIGHT,
			max_directory_entries: MAX_DIRECTORY_ENTRIES,
			max_project_page_size: MAX_PAGE_SIZE,
			image_inspection: self.image_root.is_some(),
			max_image_metadata_bytes: MAX_METADATA_BYTES,
		})
	}

	#[tool(annotations(read_only_hint = true, open_world_hint = false))]
	/// Validate a project manifest in the configured directory. Only the TOML file is read.
	async fn project_validate(
		&self,
		Parameters(request): Parameters<ProjectNameRequest>,
	) -> Result<Json<ToolReport<Validation>>, ErrorData> {
		self.local_operation(&self.project_root, Validation::Busy, move |root| {
			root.validate(&request.file_name)
		})
		.await
	}

	#[tool(annotations(read_only_hint = true, open_world_hint = false))]
	/// Read a validated manifest summary and the SHA-256 revision of the exact file bytes.
	async fn project_read(
		&self,
		Parameters(request): Parameters<ProjectNameRequest>,
	) -> Result<Json<ToolReport<Reading>>, ErrorData> {
		self.local_operation(&self.project_root, Reading::Busy, move |root| {
			root.read(&request.file_name)
		})
		.await
	}

	#[tool(annotations(read_only_hint = true, open_world_hint = false))]
	/// List candidate manifest names directly in the configured directory, in byte order.
	async fn project_list(
		&self,
		Parameters(request): Parameters<ProjectListRequest>,
	) -> Result<Json<ToolReport<Listing>>, ErrorData> {
		self.local_operation(&self.project_root, Listing::Busy, move |root| {
			root.list(request.after.as_deref(), request.page_size)
		})
		.await
	}

	#[tool(annotations(read_only_hint = true, open_world_hint = false))]
	/// Inspect a confined UDIF image. Returns a numeric container summary.
	async fn image_inspect(
		&self,
		Parameters(request): Parameters<ImageRequest>,
	) -> Result<Json<ToolReport<image::Outcome>>, ErrorData> {
		self.image_operation(request, image::Operation::Inspect)
			.await
	}

	#[tool(annotations(read_only_hint = true, open_world_hint = false))]
	/// Quickly verify a confined UDIF image's trailer and block map structure.
	async fn image_verify(
		&self,
		Parameters(request): Parameters<ImageRequest>,
	) -> Result<Json<ToolReport<image::Outcome>>, ErrorData> {
		self.image_operation(request, image::Operation::Verify)
			.await
	}
}

impl Server {
	async fn image_operation(
		&self,
		request: ImageRequest,
		operation: image::Operation,
	) -> Result<Json<ToolReport<image::Outcome>>, ErrorData> {
		let root = self.image_root.as_ref().ok_or_else(|| {
			ErrorData::invalid_request("image tools require an authorized image root", None)
		})?;
		self.local_operation(root, image::Outcome::Busy, move |root| {
			root.image_operation(&request.file_name, operation)
		})
		.await
	}

	async fn local_operation<T: Send + 'static>(
		&self,
		root: &Arc<LocalRoot>,
		busy: T,
		operation: impl FnOnce(&LocalRoot) -> T + Send + 'static,
	) -> Result<Json<ToolReport<T>>, ErrorData> {
		let outcome = match self.local_slots.clone().try_acquire_owned() {
			Ok(permit) => {
				let root = Arc::clone(root);
				tokio::task::spawn_blocking(move || {
					let _permit = permit;
					operation(&root)
				})
				.await
				.map_err(|_| ErrorData::internal_error("local worker failed", None))?
			}
			Err(_) => busy,
		};
		Ok(Json(ToolReport {
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
	let project_root = Arc::new(LocalRoot::open(&options.project_root)?);
	let image_root = options
		.image_root
		.as_deref()
		.map(LocalRoot::open)
		.transpose()?
		.map(Arc::new);
	let mut tool_router = Server::tool_router();
	if image_root.is_none() {
		tool_router.remove_route(IMAGE_INSPECT);
		tool_router.remove_route(IMAGE_VERIFY);
	}
	let server = Server {
		project_root,
		image_root,
		local_slots: Arc::new(Semaphore::new(MAX_LOCAL_OPERATIONS)),
		tool_router,
	};
	let runtime = Builder::new_current_thread().enable_time().build()?;
	runtime.block_on(async {
		server.serve(stdio()).await?.waiting().await?;
		Ok(())
	})
}
