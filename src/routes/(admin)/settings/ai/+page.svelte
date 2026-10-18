<script lang="ts">
  import { McpServerSettings } from '@ouraihub/aether-ui/mcp';
  import { ProvidersSettings } from '@ouraihub/aether-ui/providers';
  import { PROVIDER_PRESETS } from '$lib/ai/presets';
</script>

<div class="mx-auto flex max-w-4xl flex-col gap-8 p-6">
  <section class="flex flex-col gap-4">
    <header>
      <h1 class="text-2xl font-semibold tracking-tight">AI 接入</h1>
      <p class="text-muted-foreground mt-1 text-sm">
        「AI 对话」需要一个模型服务：添加服务商、填 API Key 并启用一个模型。Key 加密保存在本机。
      </p>
    </header>
    <ProvidersSettings presets={PROVIDER_PRESETS} />
  </section>

  <section class="flex flex-col gap-4">
    <header>
      <h2 class="text-lg font-semibold tracking-tight">外部 AI（MCP）</h2>
      <p class="text-muted-foreground mt-1 text-sm">
        启动本机 MCP 服务并创建令牌后，Claude、Cursor 等外部 AI 即可帮你搜索和播放歌曲；命令行用
        <code class="text-foreground">autoowl-cli</code>。服务只监听 127.0.0.1。
      </p>
    </header>
    <McpServerSettings serverName="autoowl" />
  </section>
</div>
