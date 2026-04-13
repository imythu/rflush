export function formatDate(value: string | null | undefined) {
  if (!value) return "-";
  return new Date(value).toLocaleString();
}

export function statusBadge(status: string) {
  switch (status) {
    case "success":
    case "completed":
      return "bg-emerald-100 text-emerald-700";
    case "skipped_existing":
      return "bg-slate-100 text-slate-700";
    case "failed":
      return "bg-red-100 text-red-700";
    case "running":
      return "bg-sky-100 text-sky-700";
    default:
      return "bg-violet-100 text-violet-700";
  }
}
