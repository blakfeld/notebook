import { HighlightStyle } from '@codemirror/language'
import { EditorView } from '@codemirror/view'
import { tags as t } from '@lezer/highlight'

/*
 * Every color is a token from `theme/tokens.css`, so switching themes needs
 * no editor reconfiguration. Custom properties inherit into the editor's
 * shadow root, and `@font-face` rules in the document apply there too.
 */

const PROSE_FONT =
  "'Inter Variable', system-ui, -apple-system, 'Segoe UI', sans-serif"
export const MONO_FONT =
  "'JetBrains Mono Variable', ui-monospace, SFMono-Regular, Menlo, Consolas, monospace"

/** Colors, measure, cursor, and active line: both views. */
export const editorTheme = EditorView.theme({
  '&': {
    height: '100%',
    color: 'var(--text)',
    backgroundColor: 'var(--bg)',
  },
  '.cm-scroller': {
    fontFamily: MONO_FONT,
    lineHeight: '1.6',
  },
  // A 72-character measure, centered, never closer than 16px to an edge.
  '.cm-content': {
    boxSizing: 'content-box',
    width: '100%',
    maxWidth: '72ch',
    margin: '0 auto',
    padding: '24px 16px 40vh',
    caretColor: 'var(--text)',
  },
  '.cm-cursor, .cm-dropCursor': {
    borderLeftColor: 'var(--text)',
    borderLeftWidth: '2px',
  },
  '&.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground, .cm-selectionBackground, .cm-content ::selection':
    { backgroundColor: 'var(--selection)' },
  '.cm-activeLine': { backgroundColor: 'var(--active-line)' },
  '.cm-panels': {
    backgroundColor: 'var(--surface)',
    color: 'var(--text)',
  },
  '.cm-panels.cm-panels-top': { borderBottom: '1px solid var(--border)' },
  '.cm-textfield': {
    border: '1px solid var(--control)',
    backgroundColor: 'var(--bg)',
    color: 'var(--text)',
  },
  '.cm-button': {
    backgroundImage: 'none',
    backgroundColor: 'var(--bg)',
    border: '1px solid var(--control)',
    color: 'var(--text)',
  },
  '.cm-searchMatch': { backgroundColor: 'var(--selected)' },
  '.cm-dimmed, .cm-dimmed *': { color: 'var(--dim) !important' },
  '.cm-dimmed .cm-md-task, .cm-dimmed .cm-md-hr': { opacity: '0.6' },
})

/** Typography and rendered elements: live preview only. */
export const previewTheme = EditorView.theme({
  '.cm-content': {
    fontFamily: PROSE_FONT,
    fontSize: '16px',
  },
  '.cm-md-h1': { fontSize: '1.75em', fontWeight: '700', lineHeight: '1.3' },
  '.cm-md-h2': { fontSize: '1.5em', fontWeight: '700', lineHeight: '1.3' },
  '.cm-md-h3': { fontSize: '1.3em', fontWeight: '650' },
  '.cm-md-h4': { fontSize: '1.15em', fontWeight: '650' },
  '.cm-md-h5': { fontSize: '1.05em', fontWeight: '650' },
  '.cm-md-h6': { fontSize: '1em', fontWeight: '650', color: 'var(--muted)' },
  '.cm-md-setext-mark': { lineHeight: '0.6' },
  '.cm-md-strong': { fontWeight: '700' },
  '.cm-md-em': { fontStyle: 'italic' },
  '.cm-md-strike': { textDecoration: 'line-through' },
  '.cm-md-syntax': {
    color: 'var(--syntax-marker)',
    fontWeight: '400',
    fontStyle: 'normal',
  },
  '.cm-md-code': {
    fontFamily: MONO_FONT,
    fontSize: '0.88em',
    color: 'var(--code-text)',
    backgroundColor: 'var(--code-bg)',
    borderRadius: '4px',
    padding: '0.1em 0.25em',
  },
  '.cm-md-codeblock': {
    fontFamily: MONO_FONT,
    fontSize: '0.88em',
    color: 'var(--code-text)',
    backgroundColor: 'var(--code-bg)',
    paddingInline: '0.75em !important',
  },
  '.cm-md-fence': { lineHeight: '1.2' },
  '.cm-md-quote': {
    borderLeft: '3px solid var(--control)',
    paddingLeft: '0.9em !important',
    color: 'var(--muted)',
  },
  '.cm-md-link': {
    color: 'var(--link)',
    textDecoration: 'underline',
    textUnderlineOffset: '0.15em',
  },
  '.cm-md-bullet': {
    display: 'inline-block',
    minWidth: '1ch',
    color: 'var(--muted)',
  },
  '.cm-md-hr': {
    display: 'inline-block',
    width: '100%',
    verticalAlign: 'middle',
    borderTop: '1px solid var(--control)',
  },
  '.cm-md-task': {
    margin: '0 0.4em 0 0',
    verticalAlign: '-0.1em',
    cursor: 'pointer',
    accentColor: 'var(--accent)',
  },
  '.cm-tag-pill': {
    color: 'var(--tag-fg)',
    backgroundColor: 'var(--tag-bg)',
    borderRadius: '0.6em',
    padding: '0.05em 0.35em',
  },
})

/** Syntax colors for markdown source and fenced code, from the tokens. */
export const noteHighlightStyle = HighlightStyle.define([
  { tag: t.heading, fontWeight: 'bold' },
  { tag: t.strong, fontWeight: 'bold' },
  { tag: t.emphasis, fontStyle: 'italic' },
  { tag: t.strikethrough, textDecoration: 'line-through' },
  { tag: [t.link, t.url], color: 'var(--link)' },
  { tag: t.quote, color: 'var(--muted)' },
  {
    tag: [t.processingInstruction, t.contentSeparator],
    color: 'var(--syntax-marker)',
  },
  {
    tag: [
      t.keyword,
      t.controlKeyword,
      t.moduleKeyword,
      t.operatorKeyword,
      t.definitionKeyword,
    ],
    color: 'var(--hl-keyword)',
  },
  {
    tag: [t.string, t.special(t.string), t.regexp, t.character],
    color: 'var(--hl-string)',
  },
  {
    tag: [t.comment, t.lineComment, t.blockComment],
    color: 'var(--hl-comment)',
    fontStyle: 'italic',
  },
  { tag: [t.number, t.bool, t.atom, t.null], color: 'var(--hl-number)' },
  {
    tag: [t.function(t.variableName), t.function(t.propertyName), t.macroName],
    color: 'var(--hl-function)',
  },
  {
    tag: [t.typeName, t.className, t.namespace, t.tagName],
    color: 'var(--hl-type)',
  },
  { tag: t.invalid, color: 'var(--danger)' },
])
