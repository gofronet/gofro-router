export function formatRate(value: number) {
  const units = ["бит/с", "Кбит/с", "Мбит/с", "Гбит/с"];
  let unit = 0;
  while (value >= 1000 && unit < units.length - 1) {
    value /= 1000;
    unit++;
  }
  return `${value.toLocaleString("ru-RU", { maximumFractionDigits: value >= 100 ? 0 : 1 })} ${units[unit]}`;
}

export function formatBytes(value: number | null | undefined) {
  if (value == null) return "Нет данных";
  const units = ["Б", "КБ", "МБ", "ГБ", "ТБ"];
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit++;
  }
  return `${value.toLocaleString("ru-RU", { maximumFractionDigits: unit === 0 ? 0 : 1 })} ${units[unit]}`;
}

export function formatDuration(value: number) {
  const days = Math.floor(value / 86400);
  const hours = Math.floor((value % 86400) / 3600);
  const minutes = Math.floor((value % 3600) / 60);
  if (days) return `${days} д ${hours} ч`;
  if (hours) return `${hours} ч ${minutes} мин`;
  if (minutes) return `${minutes} мин`;
  return `${Math.max(0, Math.floor(value))} сек`;
}

export function formatAgo(seconds: number | null | undefined) {
  if (seconds == null) return "Нет данных";
  if (seconds < 10) return "только что";
  if (seconds < 60) return `${Math.floor(seconds)} сек назад`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)} мин назад`;
  if (seconds < 86400) return `${Math.floor(seconds / 3600)} ч назад`;
  return `${Math.floor(seconds / 86400)} д назад`;
}

export function shortKey(key: string) {
  return key.length > 24 ? `${key.slice(0, 12)}…${key.slice(-8)}` : key;
}
