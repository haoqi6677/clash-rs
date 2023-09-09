use crate::app::router::rules::RuleMatcher;
use crate::session::Session;

#[derive(Clone)]
pub struct RuleSet {
    pub rule_set: String,
    pub target: String,
}

impl RuleMatcher for RuleSet {
    fn apply(&self, _sess: &Session) -> bool {
        false
    }

    fn target(&self) -> &str {
        self.target.as_str()
    }

    fn payload(&self) -> String {
        self.rule_set.clone()
    }

    fn type_name(&self) -> &str {
        "RuleSet"
    }
}
