// Bundled with the app as same-origin assets: no font comes from elsewhere.
import '@fontsource-variable/inter'
import '@fontsource-variable/jetbrains-mono'
import { mount } from 'svelte'
import './app.css'
import App from './App.svelte'
import { createPreferences } from './lib/preferences'
import { coreSettings, loadCore } from './platform'
import type { DeviceSettings } from './platform/settings'
import { applyTheme } from './theme/theme'

const core = loadCore()
// Reads wait for the core; if it never loads, they fail and the defaults
// apply.
const settings = core.then(coreSettings)
const device: DeviceSettings = {
  get: async (key) => (await settings).get(key),
  set: async (key, value) => (await settings).set(key, value),
}
const preferences = createPreferences(device)

// The stored theme is applied before the first frame of the app, so an
// explicit Light or Dark never flashes the other one. Until then the OS
// preference picks the colors (`tokens.css`). Not a top-level await: the
// dev core is a dynamic import, which must not wait on this module.
void preferences.load().then(() => {
  applyTheme(preferences)
  mount(App, {
    target: document.getElementById('app')!,
    props: { core, preferences },
  })
})
