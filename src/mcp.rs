use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{Implementation, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router, Json, ServerHandler,
};

use crate::{
    model::{SetupPlanRequest, TokenRequest},
    service::Service,
};

#[derive(Clone)]
pub struct McpServer {
    service: Service,
    tool_router: ToolRouter<Self>,
}

impl McpServer {
    pub fn new(service: Service) -> Self {
        Self {
            service,
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router]
impl McpServer {
    #[tool(description = "Inspect host, Charles version, and local configuration prerequisites")]
    fn doctor(&self) -> Json<crate::model::Response> {
        Json(self.service.doctor())
    }

    #[tool(description = "List immutable profile names from the configured TOML file")]
    fn profiles_list(&self) -> Json<crate::model::Response> {
        Json(self.service.profiles_list())
    }

    #[tool(description = "Validate the configured TOML profiles without changing them")]
    fn profiles_validate(&self) -> Json<crate::model::Response> {
        Json(self.service.profiles_validate())
    }

    #[tool(description = "List connected Android devices")]
    fn devices_list(&self) -> Json<crate::model::Response> {
        Json(self.service.devices_list())
    }

    #[tool(description = "Create a single-use 15-minute setup plan")]
    fn setup_plan(
        &self,
        Parameters(request): Parameters<SetupPlanRequest>,
    ) -> Json<crate::model::Response> {
        Json(self.service.setup_plan(request))
    }

    #[tool(description = "Apply a previously created setup plan")]
    fn setup_apply(
        &self,
        Parameters(request): Parameters<TokenRequest>,
    ) -> Json<crate::model::Response> {
        Json(self.service.setup_apply(&request.token))
    }

    #[tool(description = "Resume a setup after completing its manual checkpoint")]
    fn setup_resume(
        &self,
        Parameters(request): Parameters<TokenRequest>,
    ) -> Json<crate::model::Response> {
        Json(self.service.setup_resume(&request.token))
    }

    #[tool(description = "Return the active managed session, if any")]
    fn status(&self) -> Json<crate::model::Response> {
        Json(self.service.status())
    }

    #[tool(description = "Create a single-use 15-minute cleanup plan")]
    fn cleanup_plan(&self) -> Json<crate::model::Response> {
        Json(self.service.cleanup_plan())
    }

    #[tool(description = "Apply a cleanup plan using ownership and compare-and-swap guards")]
    fn cleanup_apply(
        &self,
        Parameters(request): Parameters<TokenRequest>,
    ) -> Json<crate::model::Response> {
        Json(self.service.cleanup_apply(&request.token))
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for McpServer {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.instructions = Some(
            "Manage a local Charles 4.6.8 session using immutable profiles and plan/apply guards."
                .into(),
        );
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info.server_info = Implementation::new("charles-local-mcp", env!("CARGO_PKG_VERSION"))
            .with_title("Charles Local MCP")
            .with_description("Local profile-driven Charles Proxy automation");
        info
    }
}
