pub mod device;
pub mod events;
pub mod launch;
pub mod screenshot;
pub mod tree;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperationResult {
    Success,
    NotFound(String),
}
