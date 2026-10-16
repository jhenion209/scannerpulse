import { mount } from 'svelte';
import './lib/theme.css';
import Admin from './Admin.svelte';

mount(Admin, { target: document.getElementById('app')! });
