pub mod snapshot;

pub use notist_analysis::package;
pub use notist_ir as content;
pub use notist_syntax as syntax;

pub use notist_analysis::EvaluationSession as Runtime;
pub use notist_eval::Evaluation;
pub use notist_ir::Content;

pub mod runtime {
    pub use notist_analysis::EvaluationSession as Runtime;
    pub use notist_analysis::package::relative;
    pub use notist_eval::Evaluation;
    pub use notist_ir::{Env, Value};
}
