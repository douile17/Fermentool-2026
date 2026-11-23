import { mount } from 'svelte';
import App from './App.svelte';
// Red Hat Text, bundled so the UI renders the same with no internet access.
// Latin and Latin Extended only (µ is U+00B5, in Latin); subscripts fall
// back to the system face.
import '@fontsource/red-hat-text/latin-400.css';
import '@fontsource/red-hat-text/latin-ext-400.css';
import '@fontsource/red-hat-text/latin-500.css';
import '@fontsource/red-hat-text/latin-ext-500.css';
import '@fontsource/red-hat-text/latin-600.css';
import '@fontsource/red-hat-text/latin-ext-600.css';
import '@fontsource/red-hat-text/latin-700.css';
import '@fontsource/red-hat-text/latin-ext-700.css';
// Barlow, bundled too: the run state's titles only (Overview band, sidebar).
import '@fontsource/barlow/latin-600.css';
import '@fontsource/barlow/latin-ext-600.css';
import './app.css';
import { desktopBehaviour } from './lib/desktop.js';

// the desktop window: no browser menu, no reload key, no dragged image
desktopBehaviour();

export default mount(App, { target: document.getElementById('app') });
