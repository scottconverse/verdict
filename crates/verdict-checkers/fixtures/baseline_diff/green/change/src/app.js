const express = require('express');
const app = express();

app.get('/login', (req, res) => res.send('login'));
app.get('/healthz', (req, res) => res.send('ok'));

module.exports = app;
