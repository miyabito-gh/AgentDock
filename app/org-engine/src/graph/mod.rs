//! Graph definitions for organization runs (DESIGN_ORG_GRAPH section 6). Pure data and validation:
//! no I/O, no Codex types. Reading and writing the definition files is the host's job.

pub mod def;
pub mod route;
pub mod templates;
pub mod validate;

pub use def::*;
pub use route::{select_edge, Route};
pub use templates::{builtin_templates, template_json, TEMPLATE_QUALITY_LOOP, TEMPLATE_STANDARD};
pub use validate::{validate, ActivationBound, CompiledGraph, GraphError, LoopBody, RoleInfo};
