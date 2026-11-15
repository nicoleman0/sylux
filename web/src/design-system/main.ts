import { initTypographyExperiment } from '$lib/typography-experiment';
import '../app.css';
import Catalogue from './Catalogue.svelte';
import { mount, unmount } from 'svelte';

initTypographyExperiment();

const app = mount(Catalogue, { target: document.getElementById('design-system')! });

export default { app, unmount: () => unmount(app) };
