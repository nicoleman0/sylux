<script lang="ts">
  import type { Snippet } from 'svelte';
  import BrandLogo from '$lib/BrandLogo.svelte';
  import XIcon from '@lucide/svelte/icons/x';
  import * as Sidebar from '$lib/components/ui/sidebar/index.js';
  import { Button } from '$lib/components/ui/button/index.js';

  let { instanceName, logoUrl, onhome, navigation, account }: {
    instanceName: string;
    logoUrl: string | null;
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
          <a {...props} href="#libraries" onclick={(event) => { onhome(event); close(); }}><BrandLogo url={logoUrl} /><span class="sidebar-brand group-data-[collapsible=icon]:hidden">{instanceName}</span></a>
        {/snippet}
      </Sidebar.MenuButton>
    </Sidebar.MenuItem></Sidebar.Menu>
    {#if sidebar.isMobile}<Button variant="ghost" size="icon" class="sidebar-close" aria-label="Close navigation" onclick={close}><XIcon /></Button>{/if}
  </Sidebar.Header>
  <Sidebar.Content><Sidebar.Group>{@render navigation(close)}</Sidebar.Group></Sidebar.Content>
  <Sidebar.Footer>{@render account(close)}</Sidebar.Footer>
</Sidebar.Root>
