<script lang="ts">
  import type { CoreApi, Phase } from '../../platform'
  import Recover from './Recover.svelte'
  import Register from './Register.svelte'
  import SignIn from './SignIn.svelte'

  interface Props {
    core: CoreApi
    onPhase: (phase: Phase) => void
  }

  let { core, onPhase }: Props = $props()

  let screen = $state<'sign-in' | 'register' | 'recover'>('sign-in')
  let server = $state('')
</script>

<div class="account-page">
  {#if screen === 'sign-in'}
    <SignIn
      {core}
      bind:server
      {onPhase}
      onRegister={() => (screen = 'register')}
      onRecover={() => (screen = 'recover')}
    />
  {:else if screen === 'register'}
    <Register
      {core}
      bind:server
      {onPhase}
      onSignIn={() => (screen = 'sign-in')}
    />
  {:else}
    <Recover
      {core}
      bind:server
      {onPhase}
      onSignIn={() => (screen = 'sign-in')}
    />
  {/if}
</div>
