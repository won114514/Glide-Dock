import { createRoot } from 'react-dom/client';
import App from './App';

// Keep browser menus and refresh shortcuts out of the wizard, including in preview.
document.addEventListener('contextmenu', event => event.preventDefault());
document.addEventListener('keydown', event => {
  if (event.key === 'F5' || (event.ctrlKey && !event.altKey && event.key.toLowerCase() === 'r')) {
    event.preventDefault();
  }
}, { capture: true });

createRoot(document.getElementById('root')!).render(<App />);
