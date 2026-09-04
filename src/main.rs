mod agent;
mod archive;
mod artifacts;
mod changeset;
mod cli;
mod codegraph;
mod config;
mod error;
mod external_graph;
pub mod graph;
mod import;
#[path = "learning/mod.rs"]
mod learning;
mod learning_runtime;
mod learning_schema;
mod mcp;
mod office;
mod scope;
mod secret_scan;
pub mod segment;
mod source_diff;
mod store;
mod sync;
mod sync_git;
pub mod tokenize;
mod trans;
mod trans_adapter;
mod update;
mod view;
mod work;

fn main() {
    cli::main();
}
