#[path = "../learning/mod.rs"]
mod learning;
#[path = "../learning_schema.rs"]
mod learning_schema;
#[path = "../learning/tutor.rs"]
mod tutor;
#[path = "../learning/tutor_plan.rs"]
mod tutor_plan;

fn main() {
    tutor::main();
}
