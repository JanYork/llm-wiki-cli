mod artifacts;
mod changeset;
mod cli;
mod codegraph;
mod config;
mod error;
mod external_graph;
pub mod graph;
mod import;
mod scope;
pub mod segment;
mod source_diff;
mod store;
pub mod tokenize;
mod trans;
mod view;
mod work;

fn main() {
    cli::main();
}
