<script lang="ts">
  import { memoryBridge, NoteEditor } from './editor'
  import { formatCoreVersion } from './lib/version'
  import { coreVersion } from './platform'
  import { memorySettings } from './platform/settings'

  // Placeholder until the app shell replaces this page: the editor on the
  // in-memory reference bridge, with one sample note.
  const sample = [
    '# Weekly plan',
    '',
    'Notes for #work and #side project# this week.',
    '',
    '## Groceries',
    '- eggs',
    '- milk',
    '',
    '1. first',
    '2. second',
    '',
    '- [x] done',
    '- [ ] not yet',
    '',
    '> A quote with *emphasis* and **strong** text.',
    '',
    'Not tags: C#, #42, `#code`, https://example.com/#frag',
    '',
    '```',
    '#not-a-tag inside code',
    '```',
    '',
    'A long line that wraps: ' + 'lorem ipsum dolor sit amet '.repeat(12),
  ].join('\n')

  const bridge = memoryBridge({ sample })
  const settings = memorySettings()
  let vimMode = $state(false)
  let error = $state('')
  let remoteEdits = 0

  function remoteEdit() {
    remoteEdits++
    bridge.remoteEdit('sample', (body) =>
      body.insert(0, `Edited on another device (${remoteEdits})\n`),
    )
  }

  const versionLine = coreVersion().then(formatCoreVersion)
</script>

<main>
  <header>
    <h1>Notebook</h1>
    <label><input type="checkbox" bind:checked={vimMode} /> Vim mode</label>
    <button type="button" onclick={remoteEdit}>Simulate remote edit</button>
    {#await versionLine then line}
      <span class="version">{line}</span>
    {:catch err}
      <span class="version">core unavailable: {String(err)}</span>
    {/await}
  </header>
  {#if error}
    <p class="error">{error}</p>
  {/if}
  <div class="editor">
    <NoteEditor
      {bridge}
      {settings}
      noteId="sample"
      {vimMode}
      onError={(e) => (error = String(e))}
    />
  </div>
</main>

<style>
  main {
    display: flex;
    flex-direction: column;
    height: 100vh;
  }

  header {
    display: flex;
    align-items: center;
    gap: 1rem;
    padding: 0.5rem 1rem;
    border-bottom: 1px solid var(--muted);
  }

  h1 {
    margin: 0;
    font-size: 1.1rem;
  }

  .version {
    margin-left: auto;
    color: var(--muted);
  }

  .error {
    margin: 0;
    padding: 0.25rem 1rem;
    color: #cf222e;
  }

  .editor {
    flex: 1;
    min-height: 0;
  }
</style>
