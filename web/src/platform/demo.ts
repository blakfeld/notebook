import { FakeCoreApi } from './fake'

/**
 * The fake core, open, with a few sample notes, for viewing the UI with
 * `npm run dev` in a plain browser. Dev builds only; never part of a
 * production bundle.
 */
export function demoCore(): FakeCoreApi {
  const core = new FakeCoreApi([
    {
      body: '# Weekly plan\n\nNotes for #work/acme and #home this week.\n\n- [ ] call the plumber about the sink',
      pinned: true,
    },
    { body: '# Groceries\n- eggs\n- milk\n#home' },
    { body: 'Standup notes\nShipped the tag tree. #work' },
    { body: '' },
    { body: '# Old idea\nSomething for later. #ideas', archived: true },
    { body: '# Draft\nNot needed any more.', trashed: true },
  ])
  core.phase = { phase: 'open', secure_store: true, upgrade: false }
  core.latency = 50
  return core
}
