use super::*;

impl RepositoryAccess {
    pub(super) fn coordination_session_key(context: Option<&CoordinationContext>) -> String {
        context
            .and_then(|context| context.session_id.as_deref())
            .unwrap_or("__legacy__")
            .to_string()
    }

    pub(super) fn memory_adjustment(
        &self,
        terms: &[String],
        path: &str,
        context: Option<&CoordinationContext>,
    ) -> f64 {
        let Ok(memory) = self.session_memory.lock() else {
            return 0.0;
        };
        let session_key = Self::coordination_session_key(context);
        let agent_id = context.and_then(|context| context.agent_id.as_deref());
        let session_scoped = context.is_some_and(|context| context.session_id.is_some());
        let agent_only = context
            .is_some_and(|context| context.session_id.is_none() && context.agent_id.is_some());
        let mut total = 0.0;
        let mut age = 0usize;
        for record in memory.iter().rev() {
            if record.session_id != session_key {
                continue;
            }
            if age >= 24 {
                break;
            }
            if !record.paths.iter().any(|saved| saved == path) {
                age = age.saturating_add(1);
                continue;
            }
            let overlap = terms
                .iter()
                .filter(|term| {
                    record
                        .terms
                        .iter()
                        .any(|saved| saved.as_str() == term.as_str())
                })
                .count();
            if overlap > 0 {
                let decay = 1.0 + age as f64 * 0.2;
                if session_scoped && record.agent_id.as_deref() != agent_id {
                    // Sibling agents should preferentially expose complementary evidence. Keep
                    // this penalty modest so strong lexical/semantic evidence still wins.
                    total -= (overlap as f64 * 0.9) / decay;
                } else if agent_only && record.agent_id.as_deref() != agent_id {
                    // An agent_id without a session_id gets continuity only for itself; it must not
                    // accidentally turn unrelated agent histories into positive reinforcement.
                    continue;
                } else {
                    // Same-agent continuity remains useful for follow-up queries.
                    total += (overlap as f64 * 0.55) / decay;
                }
            }
            age = age.saturating_add(1);
        }
        total.clamp(-4.0, 4.0)
    }

    pub(super) fn remember_search(
        &self,
        query: &NormalizedQuery,
        hits: &[SearchHit],
        context: Option<&CoordinationContext>,
    ) {
        let Ok(mut memory) = self.session_memory.lock() else {
            return;
        };
        let paths = hits
            .iter()
            .take(8)
            .map(|hit| hit.relative_path.clone())
            .collect::<Vec<_>>();
        if paths.is_empty() {
            return;
        }
        memory.push_back(SearchMemory {
            session_id: Self::coordination_session_key(context),
            agent_id: context.and_then(|context| context.agent_id.clone()),
            terms: query.terms.clone(),
            paths,
        });
        while memory.len() > MAX_SESSION_MEMORY_RECORDS {
            memory.pop_front();
        }
    }
}
