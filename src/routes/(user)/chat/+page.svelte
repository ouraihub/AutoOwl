<script lang="ts">
  import { goto } from '$app/navigation';
  import { ChatPanel } from '@ouraihub/aether-ui/chat';
  import { chatService } from '$lib/services/user/chat-service';

  // ChatPanel reads `model` only on mount, so resolve it first; a failure falls
  // back to no model, and the panel's own picker still works.
  const ready = chatService.defaultModel().catch(() => null);
</script>

<!-- The shell's <main> scrolls; the panel needs a fixed height to keep its composer pinned. -->
<div class="mx-auto flex h-full max-w-4xl flex-col p-6">
  {#await ready then model}
    <ChatPanel {model} purpose="music" scope="music" onOpenSettings={() => goto('/settings/ai')} />
  {/await}
</div>
