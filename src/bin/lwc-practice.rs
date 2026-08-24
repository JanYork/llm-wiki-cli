#[path = "../learning/mod.rs"]
mod learning;
#[path = "../learning_schema.rs"]
mod learning_schema;
#[path = "../learning/practice.rs"]
mod practice;

fn main() {
    practice::main();
}
