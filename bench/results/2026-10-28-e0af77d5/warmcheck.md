Aufwärmprüfung (BENCH3) · je Ziel und Endpunkt frischer Server, Aufwärmen, dann 3 × 10 s bei 16 Verbindungen; ausreichend bei höchstens 3 % Unterschied zwischen den Fenstern · Stufen 3,10,30,60 s · **angewandt für alle Seiten: 60 s**

| Ziel | Endpunkt | Aufwärmen s | Fenster Anfragen/s | Unterschied | ausreichend ab |
|---|---|---:|---|---:|---|
| opengrid-embedded-pgo-core | list | 3 | 27865 / 27781 / 28002 | 0.8 % | 3 s |
| opengrid-embedded-core | list | 3 | 27587 / 27557 / 27318 | 1.0 % | 3 s |
| laravel-frankenphp-core | list | 3 | 4742 / 4857 / 4758 | 2.4 % | 3 s |
| adonis-core | list | 3 | 16962 / 17570 / 17911 | 5.6 % |  |
| adonis-core | list | 10 | 17122 / 17548 / 17860 | 4.3 % |  |
| adonis-core | list | 30 | 17454 / 18131 / 17945 | 3.9 % |  |
| adonis-core | list | 60 | 17265 / 17271 / 14309 | 20.7 % | keine Stufe (nicht stabil) |
| goravel-core | list | 3 | 15964 / 16030 / 16039 | 0.5 % | 3 s |
| rails-threads3-core | list | 3 | 8365 / 8268 / 8162 | 2.5 % | 3 s |
| fastapi-uvicorn-core | list | 3 | 10234 / 10232 / 10064 | 1.7 % | 3 s |
| spring-platform-core | list | 3 | 17731 / 18741 / 18746 | 5.7 % |  |
| spring-platform-core | list | 10 | 18699 / 18827 / 18742 | 0.7 % | 10 s |
| dotnet-core | list | 3 | 28038 / 28195 / 28286 | 0.9 % | 3 s |
| phoenix-core | list | 3 | 6264 / 6290 / 6290 | 0.4 % | 3 s |

