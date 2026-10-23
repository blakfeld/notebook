<script lang="ts">
  import Modal from './Modal.svelte'

  interface Props {
    title: string
    message: string
    confirmLabel: string
    onConfirm: () => void
    onCancel: () => void
  }

  let { title, message, confirmLabel, onConfirm, onCancel }: Props = $props()
</script>

<Modal labelledby="confirm-title" onClose={onCancel}>
  <div class="confirm">
    <h2 id="confirm-title">{title}</h2>
    <p>{message}</p>
    <div class="buttons">
      <button type="button" data-autofocus onclick={onCancel}>Cancel</button>
      <button type="button" class="danger" onclick={onConfirm}>
        {confirmLabel}
      </button>
    </div>
  </div>
</Modal>

<style>
  .confirm {
    width: min(380px, calc(100vw - 4rem));
    padding: 1.25rem;
  }

  h2 {
    margin: 0 0 0.5rem;
    font-size: 1.1rem;
  }

  p {
    margin: 0 0 1.25rem;
  }

  .buttons {
    display: flex;
    gap: 0.5rem;
    justify-content: flex-end;
  }
</style>
