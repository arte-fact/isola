# MCP Server Development Guide

Comprehensive reference on the **Model Context Protocol (MCP)** -- the open standard for connecting LLMs to external tools and data sources.

---

## 1. What is MCP

**Model Context Protocol (MCP)** is an open protocol created by **Anthropic** (announced November 2024) that standardizes how LLM applications connect to external data sources, tools, and capabilities.

Before MCP, every AI system needed custom connectors for each data source (N x M problem). MCP reduces this to N+M by providing a universal interface -- similar to how USB standardized hardware connections, or how LSP standardized language tooling in editors.

**Governance:** In December 2025, Anthropic transferred MCP governance to the **Agentic AI Foundation (AAIF)**, a directed fund under the **Linux Foundation**, co-founded by Anthropic, Block, and OpenAI.

**Key milestones:**
- Nov 2024 -- Anthropic announces MCP
- Mar 2025 -- OpenAI adopts MCP across products (including ChatGPT)
- Apr 2025 -- Google DeepMind announces MCP support
- Dec 2025 -- Governance moves to Linux Foundation / AAIF

| Link | URL |
|------|-----|
| Specification | https://modelcontextprotocol.io/specification/2025-11-25 |
| GitHub org | https://github.com/modelcontextprotocol |
| Wikipedia | https://en.wikipedia.org/wiki/Model_Context_Protocol |

---

## 2. Architecture

### Host / Client / Server Model

| Role | Description |
|------|-------------|
| **Host** | LLM application that initiates connections (e.g. Claude Desktop, an IDE) |
| **Client** | Connector within the host that maintains a 1:1 connection with a server |
| **Server** | Service that exposes tools, resources, and prompts to the client |

### Message Format

All communication uses **JSON-RPC 2.0**. Connections are **stateful** with capability negotiation at initialization.

### Transports

| Transport | Description | Use case |
|-----------|-------------|----------|
| **stdio** | Client launches server as subprocess; JSON-RPC over stdin/stdout | Local integrations, CLI tools |
| **Streamable HTTP** | Server runs as HTTP service; single endpoint supporting POST/GET; optional SSE streaming | Remote/cloud-hosted servers |
| **SSE** *(deprecated)* | Older HTTP+SSE transport | Replaced by Streamable HTTP |

### Protocol Lifecycle

1. Client sends `initialize` with supported capabilities
2. Server responds with its capabilities
3. Client sends `initialized` notification
4. Normal message exchange
5. Either side can terminate

---

## 3. MCP Primitives

### Tools (Model-controlled)

Executable functions the AI model can invoke to perform actions.

- The model decides when/how to call them
- Require user approval before execution
- Examples: fetch from API, query database, create file, run calculation

### Resources (Application-controlled)

Structured data exposed to the AI (the "nouns"), identified by URIs.

- The client application manages and fetches resource data
- URI format: `file:///path`, `config://app`, `db://table/row`
- Content types: text or binary
- Examples: file contents, API responses, database records

### Prompts (User-controlled)

Reusable, structured message templates for common workflows.

- Users select and invoke them
- Return predefined message lists to guide model behavior
- Examples: "Summarize this code", "Generate SQL for X"

### Additional Primitives (Client-side, offered to servers)

| Primitive | Description |
|-----------|-------------|
| **Sampling** | Server asks the client's LLM to generate text |
| **Roots** | Server queries filesystem/URI boundaries |
| **Elicitation** | Server requests additional info from users (added June 2025) |
| **Tasks** | Async long-running operations (added November 2025) |

---

## 4. Building MCP Servers

### Official SDKs

| Language | Package | Tier |
|----------|---------|------|
| **TypeScript** | `@modelcontextprotocol/sdk` (npm) | 1 |
| **Python** | `mcp` (PyPI) | 1 |
| **C#** | NuGet | 1 |
| **Go** | Go module | 1 |
| **Java** | Maven (w/ Spring AI) | 2 |
| **Rust** | `rmcp` (crates.io) | 2 |
| **Swift** | GitHub | 3 |
| **Ruby** | GitHub | 3 |
| **PHP** | GitHub | 3 |
| **Kotlin** | (w/ JetBrains) | TBD |

All SDKs live under https://github.com/modelcontextprotocol.

Tier 1 = full protocol support. Tier 2 = solid, may lag on newest features. Tier 3 = community-maintained.

### Python (FastMCP)

```python
from mcp.server.fastmcp import FastMCP

mcp = FastMCP("weather")

@mcp.tool()
def get_forecast(city: str) -> str:
    """Get weather forecast for a city."""
    return f"Sunny in {city}"

@mcp.resource("config://app")
def get_config() -> str:
    return "App configuration data"

@mcp.prompt()
def review_prompt(code: str) -> str:
    return f"Please review this code:\n{code}"
```

There is also a standalone `fastmcp` package on PyPI that powers ~70% of MCP servers across all languages.

### TypeScript

```typescript
import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { z } from "zod";

const server = new McpServer({ name: "my-server", version: "1.0.0" });

server.tool("get_forecast", { city: z.string() }, async ({ city }) => {
  return { content: [{ type: "text", text: `Sunny in ${city}` }] };
});

const transport = new StdioServerTransport();
await server.connect(transport);
```

### Rust (rmcp)

```toml
# Cargo.toml
[dependencies]
rmcp = { version = "0.16", features = ["server"] }
```

```rust
#[derive(Clone)]
struct MyServer;

impl ServerHandler for MyServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            capabilities: ServerCapabilities::builder()
                .enable_resources()
                .build(),
            ..Default::default()
        }
    }
}

// stdio transport
let transport = (tokio::io::stdin(), tokio::io::stdout());
let server = service.serve(transport).await?;
```

Features: async/await on tokio, proc macros for tool definition, JSON Schema via `schemars`, type-safe argument validation.

---

## 5. MCP Consumers / Clients

| Client | Type | Notes |
|--------|------|-------|
| **Claude Desktop** | Desktop app | Deepest MCP integration; built by Anthropic |
| **Claude Code** | CLI | Native MCP with management commands |
| **ChatGPT** | Web/app | OpenAI adopted MCP in March 2025 |
| **Google Gemini** | Various | DeepMind announced support April 2025 |
| **Cursor** | AI code editor | AI-first VS Code fork |
| **Windsurf** | AI code editor | AI-native coding environment |
| **VS Code + Copilot** | IDE extension | Auto-discovers MCP servers |
| **Continue.dev** | IDE extension | Open-source AI coding assistant |
| **Cline** | VS Code extension | Step-by-step dev task handling |
| **Zed** | Editor | Built-in MCP support |
| **Replit** | Online IDE | MCP support |

The average developer now uses ~4 MCP servers per client.

---

## 6. Popular MCP Servers

### Official Reference Servers

From [modelcontextprotocol/servers](https://github.com/modelcontextprotocol/servers):

| Server | Package | Description |
|--------|---------|-------------|
| Filesystem | `@modelcontextprotocol/server-filesystem` | Secure file operations with configurable access |
| Fetch | `@modelcontextprotocol/server-fetch` | Web content fetching and conversion |
| Memory | `@modelcontextprotocol/server-memory` | Knowledge graph-based persistent memory |
| Git | `mcp-server-git` (Python) | Read, search, manipulate Git repos |
| Sequential Thinking | `@modelcontextprotocol/server-sequentialthinking` | Dynamic/reflective problem-solving |
| Everything | -- | Reference/test server |

### Notable Integration Servers

| Server | Repo/Package | Description |
|--------|-------------|-------------|
| GitHub | `github/github-mcp-server` | PRs, issues, code search, reviews |
| Slack | -- | Channel management, messaging |
| PostgreSQL | -- | Direct database querying |
| SQLite | -- | Database interaction |
| Docker | -- | Docker daemon operations |
| Azure | -- | Azure cloud resource management |
| Playwright | -- | Browser automation |
| Brave Search | -- | Web search |
| Google Maps | -- | Location services |
| Sentry | -- | Error tracking |

### Client Configuration Example (Claude Desktop)

```json
{
  "mcpServers": {
    "memory": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-memory"]
    },
    "filesystem": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-filesystem", "/path/to/files"]
    },
    "github": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-github"],
      "env": { "GITHUB_PERSONAL_ACCESS_TOKEN": "<TOKEN>" }
    }
  }
}
```

### Curated Lists

- https://github.com/modelcontextprotocol/servers (official)
- https://github.com/wong2/awesome-mcp-servers
- https://mcpservers.org/

---

## 7. Deployment & Hosting

### Local (stdio)

Server runs as a subprocess launched by the client. No network needed. Most common for development.

Works with `npx`, `uvx`, `pip install`, or compiled binaries.

### Remote (Streamable HTTP)

Server runs as an independent HTTP service on a single endpoint. Handles multiple clients. Can use SSE for streaming.

### Hosting Platforms

| Platform | Notes |
|----------|-------|
| **Docker** | Primary deployment vehicle; Docker MCP Toolkit for catalog/management |
| **Google Cloud Run** | Native support for remote MCP servers with Streamable HTTP |
| **Azure Container Apps** | Official Microsoft guidance available |
| **AWS ECS / Fargate** | Container-based deployment |
| **Cloudflare Workers** | Edge/serverless MCP hosting with dedicated transport docs |
| **Railway / Fly.io** | Simple cloud / edge deployment |
| **Kubernetes** | Tools like `kmcp` generate manifests |
| **mcp.run** | Servers compiled to WebAssembly for secure, portable execution |

| Link | URL |
|------|-----|
| Docker guide | https://www.docker.com/blog/build-to-prod-mcp-servers-with-docker/ |
| Cloud Run guide | https://cloud.google.com/blog/topics/developers-practitioners/build-and-deploy-a-remote-mcp-server-to-google-cloud-run-in-under-10-minutes |
| Azure guide | https://techcommunity.microsoft.com/blog/appsonazureblog/host-remote-mcp-servers-in-azure-container-apps/4403550 |

---

## 8. Authentication & Security

### OAuth 2.1

MCP uses OAuth 2.1 for authorization, with significant evolution across spec versions:

| Spec version | Auth changes |
|--------------|-------------|
| 2025-03-26 | Initial OAuth 2.1 framework alongside Streamable HTTP |
| 2025-06-18 | Split MCP server from auth server; added Protected Resource Metadata (RFC 9728) and Resource Indicators |
| 2025-11-25 | Client ID Metadata Documents (CIMD) as primary registration; **mandatory PKCE** for all clients |

### Key Security Features

- **PKCE** -- mandatory, prevents authorization code interception
- **Scoped access control** -- fine-grained permissions
- **Short-lived tokens** -- reduced exposure window
- **Protected Resource Metadata discovery** -- servers advertise auth requirements
- **OpenID Connect** support for authorization server resolution

### Security Principles

1. **User Consent and Control** -- explicit consent for all data access
2. **Data Privacy** -- explicit consent before exposing data to servers
3. **Tool Safety** -- tools represent arbitrary code execution; annotations/descriptions are untrusted unless from a trusted server
4. **LLM Sampling Controls** -- users must approve sampling requests

### Known Vulnerabilities

- **CVE-2025-6514**: OAuth proxy vulnerability in `mcp-remote` (fixed in v0.1.16+)
- CSRF-style attacks on improperly bound OAuth state
- Consent binding issues in some server implementations

| Link | URL |
|------|-----|
| Auth tutorial | https://modelcontextprotocol.io/docs/tutorials/security/authorization |
| OAuth guide | https://www.scalekit.com/blog/implement-oauth-for-mcp-servers |

---

## 9. MCP Server Registries

| Registry | URL | Notes |
|----------|-----|-------|
| **Official MCP Registry** | https://registry.modelcontextprotocol.io | Centralized metadata; namespace auth (reverse DNS); backed by Anthropic, GitHub, Microsoft |
| **GitHub MCP Registry** | GitHub blog | GitHub's own discovery layer |
| **Smithery** | https://smithery.ai | Discover, install, manage MCP servers |
| **Glama.ai** | https://glama.ai/mcp/servers | Comprehensive registry with API access |
| **mcp.run** | https://mcp.run | Wasm-based hosting; multi-language |
| **PulseMCP** | https://www.pulsemcp.com | Server directory |
| **mcpservers.org** | https://mcpservers.org | Community "Awesome" directory |

---

## 10. Specification Version History & Roadmap

### Version History

| Version | Date | Key Changes |
|---------|------|-------------|
| 2024-11-05 | Nov 2024 | Initial release |
| 2025-03-26 | Mar 2025 | OAuth 2.1; Streamable HTTP replaces SSE; tool annotations; JSON-RPC batching |
| 2025-06-18 | Jun 2025 | Structured tool outputs; enhanced OAuth; elicitation; batching removed |
| 2025-11-25 | Nov 2025 | Tasks primitive (async ops); CIMD; mandatory PKCE; auth overhaul |

### 2026 Roadmap

Top priorities with expedited review:

1. **Streamable HTTP improvements** -- stateful sessions vs load balancers, horizontal scaling, standardized server capability discovery
2. **Tasks lifecycle improvements** -- retry semantics, expiry policies for completed results
3. **Enterprise readiness** -- audit trails, SSO-integrated auth, gateway behavior, configuration portability

Next spec release tentatively slated for **June 2026**.

| Link | URL |
|------|-----|
| 2026 Roadmap | https://blog.modelcontextprotocol.io/posts/2026-mcp-roadmap/ |
| MCP Blog | https://blog.modelcontextprotocol.io/ |
| Changelog | https://modelcontextprotocol.io/specification/2025-03-26/changelog |

---

*Research compiled April 2026.*
