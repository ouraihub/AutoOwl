<script lang="ts" module>
  import type { SourceId } from '$lib/domain/contracts';

  /** Chinese display name per source id. */
  export const SOURCE_NAMES: Record<string, string> = {
    netease: '网易云',
    qq: 'QQ音乐',
    kugou: '酷狗',
    kuwo: '酷我',
    migu: '咪咕',
    qianqian: '千千',
    soda: '汽水音乐',
    apple: 'Apple Music',
    bilibili: '哔哩哔哩',
    fivesing: '5sing',
    jamendo: 'Jamendo',
    joox: 'JOOX'
  };

  /** Falls back to the raw id so an unknown source never renders blank. */
  export function sourceName(source: SourceId): string {
    return SOURCE_NAMES[source] ?? source;
  }
</script>

<script lang="ts">
  import { Badge } from '$lib/components/ui/badge/index.js';
  import { cn } from '$lib/utils.js';

  let {
    source,
    size = 'md',
    class: className
  }: { source: SourceId; size?: 'sm' | 'md'; class?: string } = $props();

  const label = $derived(sourceName(source));
</script>

<Badge
  variant="secondary"
  class={cn(size === 'sm' ? 'h-4 px-1.5 text-[10px]' : 'h-5 px-2 text-xs', className)}
>
  {label}
</Badge>
