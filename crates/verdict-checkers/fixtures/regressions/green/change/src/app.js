const { slug } = require('./util');

export function a() {
  return slug('A b');
}

export async function load(url) {
  try {
    return await fetch(url);
  } catch (e) {
    console.error('load failed', e);
    throw e;
  }
}

const key = process.env.API_KEY;
const model = trainer.fit(data);
const sample_password = process.env.DB_PASSWORD || "";
