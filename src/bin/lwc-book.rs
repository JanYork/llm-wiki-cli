#[path = "../learning/book.rs"]
mod book;
#[path = "../learning/mod.rs"]
mod learning;
#[path = "../learning_schema.rs"]
mod learning_schema;
#[path = "../trans_adapter.rs"]
mod trans_adapter;

fn main() {
    book::main();
}
