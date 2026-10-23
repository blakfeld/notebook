import { mount } from 'svelte'
import './app.css'
import App from './App.svelte'
import { loadCore } from './platform'

const app = mount(App, {
  target: document.getElementById('app')!,
  props: { core: loadCore() },
})

export default app
