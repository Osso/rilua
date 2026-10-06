//! Lazy private revocation sidecar, preserving the public CallInfo representation.

use super::{LuaState, LuaThread};
use crate::vm::gc::arena::GcRef;

#[derive(Default)]
pub(super) struct SecretAccessContexts {
    revoked: Vec<(Option<GcRef<LuaThread>>, usize)>,
    resumers: Vec<(usize, Option<GcRef<LuaThread>>)>,
}

impl SecretAccessContexts {
    // Frame operations are hot, but these vector updates are needed only after
    // the host first revokes access. Keep them out of ordinary frame dispatch.
    #[cold]
    #[inline(never)]
    fn prune(&mut self, current: Option<GcRef<LuaThread>>, depth: usize) {
        self.revoked
            .retain(|(thread, revoked)| *thread != current || *revoked <= depth);
    }

    #[cold]
    #[inline(never)]
    fn transfer_tail(&mut self, current: Option<GcRef<LuaThread>>, depth: usize, previous: usize) {
        for (thread, revoked) in &mut self.revoked {
            if *thread == current && *revoked == depth {
                *revoked = previous;
            }
        }
    }

    #[cold]
    #[inline(never)]
    fn push_resumer(&mut self, index: usize, current: Option<GcRef<LuaThread>>) {
        // Before a revocation, ancestors have no denial to inherit.
        if !self.revoked.is_empty() {
            self.resumers.push((index, current));
        }
    }

    #[cold]
    #[inline(never)]
    fn pop_resumer(
        &mut self,
        index: usize,
        current: Option<GcRef<LuaThread>>,
        context_ended: bool,
    ) {
        if self.resumers.last().is_some_and(|(slot, _)| *slot == index) {
            self.resumers.pop();
        }
        if context_ended {
            self.revoked.retain(|(thread, _)| *thread != current);
        }
    }
}

impl LuaState {
    pub(crate) fn mark_secret_context_revoked(&mut self, depth: usize) {
        let contexts = self.secret_access_contexts.get_or_insert_with(Box::default);
        if let Some((_, previous)) = contexts
            .revoked
            .iter_mut()
            .find(|(thread, _)| *thread == self.current_thread)
        {
            *previous = (*previous).min(depth);
        } else {
            contexts.revoked.push((self.current_thread, depth));
        }
    }

    pub(crate) fn secret_context_is_revoked(&self) -> bool {
        let Some(contexts) = &self.secret_access_contexts else {
            return false;
        };
        contexts.revoked.iter().any(|(thread, depth)| {
            (*thread == self.current_thread && *depth <= self.ci)
                || contexts.resumers.iter().any(|(index, resumer)| {
                    thread == resumer
                        && self
                            .saved_threads
                            .get(*index)
                            .is_some_and(|state| *depth <= state.ci)
                })
        })
    }

    /// Prune frames unwound directly by pcall/host APIs before a slot is reused.
    #[inline]
    pub(crate) fn prune_secret_contexts(&mut self) {
        if let Some(contexts) = &mut self.secret_access_contexts {
            contexts.prune(self.current_thread, self.ci);
        }
    }

    #[inline]
    pub(crate) fn transfer_tail_secret_context(&mut self, previous_depth: usize) {
        if let Some(contexts) = &mut self.secret_access_contexts {
            contexts.transfer_tail(self.current_thread, self.ci, previous_depth);
        }
    }

    #[inline]
    pub(crate) fn push_secret_resumer(&mut self) {
        if let Some(contexts) = &mut self.secret_access_contexts {
            let index = self.saved_threads.len().saturating_sub(1);
            contexts.push_resumer(index, self.current_thread);
        }
    }

    #[inline]
    pub(crate) fn pop_secret_resumer(&mut self, context_ended: bool) {
        if let Some(contexts) = &mut self.secret_access_contexts {
            let index = self.saved_threads.len().saturating_sub(1);
            contexts.pop_resumer(index, self.current_thread, context_ended);
        }
    }
}
