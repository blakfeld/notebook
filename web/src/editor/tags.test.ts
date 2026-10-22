// @vitest-environment jsdom
/// <reference types="node" />
import { readFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { forceParsing } from '@codemirror/language'
import type { EditorView } from '@codemirror/view'
import { describe, expect, it } from 'vitest'
import { findTagRanges, tagHighlighter } from './tags'
import { cursorAtEnd, setup, type } from './testing'

interface TagCase {
  body: string
  tags: string[]
}

// Shared with the Rust tests. Read from disk: it lives outside the Vite root.
const fixture = resolve(
  dirname(fileURLToPath(import.meta.url)),
  '../../../crates/notebook-core/tests/fixtures/tag_cases.json',
)
const cases: TagCase[] = JSON.parse(readFileSync(fixture, 'utf8'))

/** The text of every `cm-tag` mark in the editor, in order. */
function highlighted(view: EditorView): string[] {
  forceParsing(view, view.state.doc.length, 5000)
  const found: string[] = []
  const decorations = view.plugin(tagHighlighter)?.decorations
  decorations?.between(0, view.state.doc.length, (from, to, value) => {
    if (value.spec.class === 'cm-tag') found.push(view.state.sliceDoc(from, to))
  })
  return found
}

async function editorWith(text: string) {
  const { editor, view } = setup({ a: text })
  await editor.openNote('a')
  return view
}

describe('tag highlighting', () => {
  it('has fixture cases', () => {
    expect(cases.length).toBeGreaterThan(50)
  })

  it.each(cases)('matches the Rust parser on $body', async ({ body, tags }) => {
    expect(highlighted(await editorWith(body))).toEqual(tags)
  })

  it('single-word and multi-word tags', async () => {
    const view = await editorWith('Ideas for #side project# and #work/acme.')
    expect(highlighted(view)).toEqual(['#side project#', '#work/acme'])
    expect(view.contentDOM.querySelector('.cm-tag')?.textContent).toBe(
      '#side project#',
    )
  })

  it('non-tags are not highlighted', async () => {
    const view = await editorWith(
      '# Heading\nC# and #42 and `#code` and https://x.io/#frag',
    )
    expect(highlighted(view)).toEqual([])
  })

  it('highlighting follows edits', async () => {
    const view = await editorWith('')
    cursorAtEnd(view)
    type(view, '#todo')
    type(view, ' ')
    expect(highlighted(view)).toEqual(['#todo'])
  })
})

describe('findTagRanges', () => {
  it('skips excluded ranges', () => {
    expect(findTagRanges('#a `#b` #c', [[3, 7]])).toEqual([
      [0, 2],
      [8, 10],
    ])
  })

  it('stops a tag at the start of an excluded range', () => {
    expect(findTagRanges('#a b#`c`', [[5, 8]])).toEqual([[0, 5]])
  })

  it('uses UTF-16 offsets', () => {
    expect(findTagRanges('😀 #fun')).toEqual([[3, 7]])
  })
})
