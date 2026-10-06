//! Private revocation sidecar, preserving the public CallInfo representation.

use super::{LuaState, LuaThread};
use crate::vm::gc::arena::GcRef;

#[derive(Default)]
pub(super) struct SecretAccessContexts {
    revoked: Vec<(Option<GcRef<LuaThread>>, usize)>,
    resumers: Vec<Option<GcRef<LuaThread>>>,
}

impl LuaState {
    pub(crate) fn mark_secret_context_revoked(&mut self, depth: usize) {
        if let Some((_, previous)) = self
            .secret_access_contexts
            .revoked
            .iter_mut()
            .find(|(thread, _)| *thread == self.current_thread)
        {
            *previous = (*previous).min(depth);
        } else {
            self.secret_access_contexts
                .revoked
                .push((self.current_thread, depth));
        }
    }

    pub(crate) fn secret_context_is_revoked(&self) -> bool {
        self.secret_access_contexts
            .revoked
            .iter()
            .any(|(thread, depth)| {
                (*thread == self.current_thread && *depth <= self.ci)
                    || self
                        .secret_access_contexts
                        .resumers
                        .iter()
                        .zip(&self.saved_threads)
                        .any(|(resumer, state)| thread == resumer && *depth <= state.ci)
            })
    }

    /// Prune frames unwound directly by pcall/host APIs before a slot is reused.
    #[inline]
    pub(crate) fn prune_secret_contexts(&mut self) {
        if self.secret_access_contexts.revoked.is_empty() {
            return;
        }
        let current = self.current_thread;
        let depth = self.ci;
        self.secret_access_contexts
            .revoked
            .retain(|(thread, revoked)| *thread != current || *revoked <= depth);
    }

    pub(crate) fn transfer_tail_secret_context(&mut self, previous_depth: usize) {
        for (thread, depth) in &mut self.secret_access_contexts.revoked {
            if *thread == self.current_thread && *depth == self.ci {
                *depth = previous_depth;
            }
        }
    }

    pub(crate) fn push_secret_resumer(&mut self) {
        self.secret_access_contexts
            .resumers
            .push(self.current_thread);
    }

    pub(crate) fn pop_secret_resumer(&mut self, context_ended: bool) {
        self.secret_access_contexts.resumers.pop();
        if context_ended {
            let current = self.current_thread;
            self.secret_access_contexts
                .revoked
                .retain(|(thread, _)| *thread != current);
        }
    }
}
