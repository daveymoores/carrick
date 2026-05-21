import { useEffect, useState } from 'react';

export function LegacyWidget() {
  const [order, setOrder] = useState(null);

  useEffect(() => {
    fetch('/api/orders/latest')
      .then((res) => res.json())
      .then(setOrder);
  }, []);

  const submit = async () => {
    await fetch('/api/orders', {
      method: 'POST',
      body: JSON.stringify({ ok: true }),
    });
  };

  return (
    <div>
      <pre>{JSON.stringify(order)}</pre>
      <button onClick={submit}>Reorder</button>
    </div>
  );
}
