import React from 'react';
import ReactDOM from 'react-dom/client';
import App from './App';
import './index.css';
import { useAppStore } from './stores/appStore';
import { apiGet, apiPost, apiPostFormData } from './api/client';

(window as any).__FUSIONHUB__ = {
  React,
  ReactDOM,
  useAppStore,
  api: { apiGet, apiPost, apiPostFormData },
  registerPage: (id: string, component: React.ComponentType) => {
    useAppStore.getState().registerExtensionComponent(id, component);
  },
};

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
