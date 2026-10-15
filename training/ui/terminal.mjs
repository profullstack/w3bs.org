// Keyboard-first terminal review on @profullstack/hqtui. `reviewView` is a
// pure function of state so tests render it headlessly.
import { createApp, editText, insertText } from '@profullstack/hqtui';

const DECISIONS = { c: 'complete', p: 'partial', f: 'failed', i: 'interrupted', u: 'unknown' };

export function reviewView(state) {
  return ({ ui, theme }) => {
    const card = state.cards[state.selected];
    const detail = state.detail;
    ui.column({ gap: 0 }, (page) => {
      page.row({ size: '1fr', gap: 1 }, (row) => {
        row.panel(
          {
            title: `Inbox ${state.cards.length ? state.selected + 1 : 0}/${state.cards.length}`,
            subtitle: `budget ${state.budget}`,
            width: '40%',
          },
          (p) => {
            if (!state.cards.length) p.text('Nothing to review.', { dim: true });
            else
              p.list({
                items: state.cards.map((c) => ({
                  label: `${c.privacy_warnings ? '! ' : ''}${String(c.goal.value).slice(0, 60)}`,
                  badge: c.goal.status === 'confirmed' ? 'goal ok' : 'goal?',
                  color: c.privacy_warnings ? theme.danger : undefined,
                })),
                selected: state.selected,
                followSelection: true,
              });
          },
        );
        row.panel({ title: card ? 'Task' : 'Task', size: '1fr' }, (p) => {
          if (!card) return;
          p.keyValues([
            {
              label: 'Goal',
              value: `${card.goal.value} (${card.goal.status}, ${card.goal.source})`,
            },
            { label: 'Result', value: `${card.outcome.value} (${card.outcome.status})` },
            {
              label: 'Steps',
              value: `${card.observed_steps} observed, ${card.uncertain_labels} uncertain`,
            },
            {
              label: 'Privacy',
              value: card.privacy_warnings
                ? `${card.privacy_warnings} warning(s): review before approval`
                : 'no warnings',
              color: card.privacy_warnings ? theme.danger : theme.success,
            },
            { label: 'Profile', value: card.capture_profile },
            { label: 'Why now', value: card.reasons.join(', ') || 'sampled' },
          ]);
          if (detail?.warnings?.length)
            p.text(
              detail.warnings
                .filter((w) => !w.resolved_at)
                .map((w) => `! ${w.kind} at ${w.path}`)
                .join('\n'),
              { color: theme.danger },
            );
          if (detail?.steps?.length)
            p.text(
              detail.steps
                .slice(0, 12)
                .map(
                  (s, n) =>
                    `${String(n + 1).padStart(2)}. ${s.action.type} ${[s.action.target?.app, s.action.target?.role, s.action.target?.name].filter(Boolean).join(' / ')}${s.alignment.status === 'verified' ? '' : ' (uncertain)'}${s.action.status === 'confirmed' ? '' : ' [inferred]'}`,
                )
                .join('\n'),
              { dim: false },
            );
        });
      });
      if (state.mode === 'rename') {
        page.panel({ title: 'Rename goal (Enter saves, Esc cancels)', height: 3 }, (p) =>
          p.textInput({ value: state.field.value, cursor: state.field.cursor, focused: true }),
        );
      }
      if (state.message)
        page.text(state.message, { color: state.error ? theme.danger : theme.success });
      page.statusBar({
        items: [
          { key: 'c', label: 'Complete' },
          { key: 'p', label: 'Partial' },
          { key: 'f', label: 'Failed' },
          { key: 'u', label: 'Unknown' },
          { key: 'r', label: 'Rename' },
          { key: 'x', label: 'Clear privacy' },
          { key: 'd', label: state.confirmDiscard ? 'Discard? d again' : 'Discard' },
          { key: 'q', label: 'Quit' },
        ],
      });
    });
  };
}

export async function reviewLoop(service, actor) {
  const state = {
    cards: [],
    selected: 0,
    budget: 0,
    detail: null,
    mode: 'list',
    field: { value: '', cursor: 0 },
    message: '',
    error: false,
    confirmDiscard: false,
  };
  const refresh = async () => {
    const inbox = await service.inbox();
    state.cards = inbox.cards;
    state.budget = inbox.budget;
    state.selected = Math.min(state.selected, Math.max(0, state.cards.length - 1));
    state.detail = state.cards[state.selected]
      ? await service.getEpisode(state.cards[state.selected].episode_id)
      : null;
  };
  const act = async (input) => {
    const card = state.cards[state.selected];
    if (!card) return;
    try {
      const result = await service.review(
        card.episode_id,
        { ...input, expected_revision: card.revision },
        actor,
      );
      state.message =
        input.decision === 'discard' ? 'Discarded and deleted.' : `Saved: ${result.state}.`;
      state.error = false;
    } catch (error) {
      state.message = `${error.code}: ${error.message}`;
      state.error = true;
    }
    await refresh();
  };
  await refresh();
  const app = await createApp({ quitKeys: ['ctrl+c'] });
  app.on('paste', (text) => {
    if (state.mode === 'rename') state.field = insertText(state.field, text);
  });
  app.on('key', async (event) => {
    if (state.mode === 'rename') {
      if (event.name === 'escape') state.mode = 'list';
      else if (event.name === 'enter') {
        state.mode = 'list';
        await act({ decision: 'rename', goal: state.field.value });
      } else state.field = editText(state.field, event) ?? state.field;
      return;
    }
    const key = event.key;
    if (key !== 'd') state.confirmDiscard = false;
    if (key === 'q') app.quit();
    else if (event.name === 'down' || key === 'j') {
      state.selected = Math.min(state.cards.length - 1, state.selected + 1);
      await refresh();
    } else if (event.name === 'up' || key === 'k') {
      state.selected = Math.max(0, state.selected - 1);
      await refresh();
    } else if (DECISIONS[key]) await act({ decision: DECISIONS[key], confirm_goal: true });
    else if (key === 'x') await act({ decision: 'clear_privacy' });
    else if (key === 'r' && state.cards[state.selected]) {
      state.mode = 'rename';
      const value = String(state.cards[state.selected].goal.value);
      state.field = { value, cursor: value.length };
    } else if (key === 'd') {
      if (state.confirmDiscard) {
        state.confirmDiscard = false;
        await act({ decision: 'discard' });
      } else state.confirmDiscard = true;
    }
  });
  app.render((args) => reviewView(state)(args));
  await app.start();
}
