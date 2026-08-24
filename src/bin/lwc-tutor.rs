#[path = "../learning/mod.rs"]
mod learning;
#[path = "../learning/tutor.rs"]
mod tutor;
#[path = "../learning/tutor_plan.rs"]
mod tutor_plan;

fn main() {
    tutor::main();
}
