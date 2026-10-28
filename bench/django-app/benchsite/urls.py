"""Routes of the Django comparison app (BENCH3), as in the Laravel app."""

from django.urls import path

from bench import views

urlpatterns = [
    path("health", views.health),
    path("api/bench/sql/health", views.health),
    path("api/bench/sql/sqlite", views.sqlite_info),
    path("api/bench/sql/login", views.login),
    path("api/bench/sql/notes", views.notes),
    path("api/bench/sql/notes/<str:note_id>", views.show),
    path("api/bench/logic/hydrate", views.hydrate),
    path("api/bench/logic/collect", views.collect),
    path("api/bench/logic/validate", views.validate),
    path("api/bench/logic/json", views.json_),
]
