import { initTypographyExperiment } from '$lib/typography-experiment';
import { mount, unmount } from 'svelte';
import App from './App.svelte';

initTypographyExperiment();

const app = mount(App, { target: document.getElementById('app')! });

export default { app, unmount: () => unmount(app) };
