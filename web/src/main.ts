import { mount } from 'svelte';
import './lib/theme.css';
import App from './App.svelte';

mount(App, { target: document.getElementById('app')! });
