<script lang="ts">
  import type { Snippet } from 'svelte';
  import FolderOpenIcon from '@lucide/svelte/icons/folder-open';
  import XIcon from '@lucide/svelte/icons/x';
  import * as Sidebar from '$lib/components/ui/sidebar/index.js';
  import { Button } from '$lib/components/ui/button/index.js';

  let { instanceName, onhome, navigation, account }: {
    instanceName: string;
    onhome: (event: MouseEvent) => void;
    navigation: Snippet<[() => void]>;
    account: Snippet<[() => void]>;
  } = $props();
  const sidebar = Sidebar.useSidebar();
  const close = () => sidebar.setOpenMobile(false);
</script>

<Sidebar.Root collapsible="icon" variant="inset">
  <Sidebar.Header>
    <Sidebar.Menu><Sidebar.MenuItem>
      <Sidebar.MenuButton size="lg" tooltipContent={instanceName} aria-label={instanceName}>
        {#snippet child({ props })}
          <a {...props} href="#files" onclick={(event) => { onhome(event); close(); }}><FolderOpenIcon /><span class="sidebar-brand group-data-[collapsible=icon]:hidden">{instanceName}</span></a>
        {/snippet}
      </Sidebar.MenuButton>
    </Sidebar.MenuItem></Sidebar.Menu>
    {#if sidebar.isMobile}<Button variant="ghost" size="icon" class="sidebar-close" aria-label="Close navigation" onclick={close}><XIcon /></Button>{/if}
  </Sidebar.Header>
  <Sidebar.Content><Sidebar.Group>{@render navigation(close)}</Sidebar.Group></Sidebar.Content>
  <Sidebar.Footer>{@render account(close)}</Sidebar.Footer>
  <Sidebar.Rail />
</Sidebar.Root>
