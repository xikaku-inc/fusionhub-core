import { useEffect, useRef, useState } from 'react';
import { useAppStore } from '../../stores/appStore';
import type { UiExtension } from '../../types/nodes';

interface Props {
  extension: UiExtension;
}

export default function ExtensionLoader({ extension }: Props) {
  const [Component, setComponent] = useState<React.ComponentType | null>(() => {
    const ext = useAppStore.getState().uiExtensions.find((e) => e.id === extension.id);
    return ext?.component ?? null;
  });
  const loadedRef = useRef(false);

  useEffect(() => {
    if (Component) return;

    const unsub = useAppStore.subscribe((state) => {
      const ext = state.uiExtensions.find((e) => e.id === extension.id);
      if (ext?.component) {
        setComponent(() => ext.component!);
      }
    });

    if (!loadedRef.current) {
      loadedRef.current = true;
      const script = document.createElement('script');
      script.src = `/ui-ext/${extension.id}.js`;
      document.head.appendChild(script);
    }

    return () => unsub();
  }, [extension.id, Component]);

  if (!Component) {
    return <div style={{ padding: 32, color: '#888' }}>Loading {extension.displayName}...</div>;
  }

  return <Component />;
}
