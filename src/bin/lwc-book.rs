#[path = "../learning/book.rs"]
mod book;
#[path = "../learning/mod.rs"]
mod learning;
#[path = "../trans_adapter.rs"]
mod trans_adapter;

fn main() {
    book::main();
}
