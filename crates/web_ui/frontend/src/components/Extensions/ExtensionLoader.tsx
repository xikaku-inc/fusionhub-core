import { useEffect, useState } from 'react';
import { useAppStore } from '../../stores/appStore';
import type { UiExtension } from '../../types/nodes';

interface Props {
  extension: UiExtension;
}

// Track loaded bundle scripts globally to avoid duplicate loads
const loadedBundles = new Set<string>();

export default function ExtensionLoader({ extension }: Props) {
  const bundleId = extension.bundleId || extension.id;

  const [Component, setComponent] = useState<React.ComponentType<any> | null>(() => {
    const ext = useAppStore.getState().uiExtensions.find((e) => e.id === bundleId);
    return ext?.component ?? null;
  });

  useEffect(() => {
    if (Component) return;

    const unsub = useAppStore.subscribe((state) => {
      const ext = state.uiExtensions.find((e) => e.id === bundleId);
      if (ext?.component) {
        setComponent(() => ext.component!);
      }
    });

    if (!loadedBundles.has(bundleId)) {
      loadedBundles.add(bundleId);
      const script = document.createElement('script');
      script.src = `/ui-ext/${bundleId}.js`;
      document.head.appendChild(script);
    }

    return () => unsub();
  }, [bundleId, Component]);

  if (!Component) {
    return <div style={{ padding: 32, color: '#888' }}>Loading {extension.displayName}...</div>;
  }

  return <Component extension={extension} />;
}
