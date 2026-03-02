export async function apiGet<T = any>(url: string): Promise<T> {
  const r = await fetch(url);
  if (!r.ok) throw new Error(`GET ${url} failed: ${r.status}`);
  return r.json();
}

export async function apiPost<T = any>(url: string, body?: any): Promise<T> {
  const r = await fetch(url, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body ?? {}),
  });
  if (!r.ok) throw new Error(`POST ${url} failed: ${r.status}`);
  return r.json();
}

export async function apiPostFormData<T = any>(url: string, formData: FormData): Promise<T> {
  const r = await fetch(url, { method: 'POST', body: formData });
  if (!r.ok) throw new Error(`POST ${url} failed: ${r.status}`);
  return r.json();
}
