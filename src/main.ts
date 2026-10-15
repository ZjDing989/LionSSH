import { createApp } from 'vue';
import App from './App.vue';
import './style.css';

// Keep the webview's browser menu out of the app without intercepting terminal input.
document.addEventListener('contextmenu', (event) => event.preventDefault(), { capture: true });

createApp(App).mount('#app');
document.getElementById('startup-shell')?.remove();
