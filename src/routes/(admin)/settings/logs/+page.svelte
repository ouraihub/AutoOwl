<script lang="ts">
  import { LogPanel, LogSettings } from '@ouraihub/aether-ui/logs';
</script>

<div class="mx-auto flex max-w-3xl flex-col gap-6 p-6">
  <header>
    <h1 class="text-2xl font-semibold tracking-tight">日志</h1>
    <p class="text-muted-foreground mt-1 text-sm">
      写入数据目录下的 <code class="text-foreground">logs/autoowl.log</code>，单文件 5 MB，保留 3 份。
    </p>
  </header>
  <LogSettings />
  <LogPanel />
</div>
