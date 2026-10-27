const { slug } = require('./util');

export function a() {
  return slug('A b');
}

export function load(url) {
  try {
    fetch(url);
  } catch (e) {
  }
}

debugger;

const awsKey = "AKIAIOSFODNN7EXAMPLE";
const db_password = "hunter2-Correct-Horse-9";
