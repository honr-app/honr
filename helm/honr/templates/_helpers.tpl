{{/*
Expand the name of the chart.
*/}}
{{- define "honr.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Fully qualified app name.
*/}}
{{- define "honr.fullname" -}}
{{- if .Values.fullnameOverride }}
{{- .Values.fullnameOverride | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- $name := default .Chart.Name .Values.nameOverride }}
{{- if contains $name .Release.Name }}
{{- .Release.Name | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- printf "%s-%s" .Release.Name $name | trunc 63 | trimSuffix "-" }}
{{- end }}
{{- end }}
{{- end }}

{{/*
Common labels
*/}}
{{- define "honr.labels" -}}
helm.sh/chart: {{ include "honr.name" . }}-{{ .Chart.Version | replace "+" "_" }}
app.kubernetes.io/name: {{ include "honr.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- end }}

{{/*
Selector labels for the honr Deployment
*/}}
{{- define "honr.selectorLabels" -}}
app.kubernetes.io/name: {{ include "honr.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/component: board
{{- end }}

{{/*
Selector labels for Postgres
*/}}
{{- define "honr.postgres.selectorLabels" -}}
app.kubernetes.io/name: {{ include "honr.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/component: postgres
{{- end }}

{{/*
Secret name holding master key + database URL
*/}}
{{- define "honr.secretName" -}}
{{ include "honr.fullname" . }}-secrets
{{- end }}

{{/*
Postgres Service DNS name (in-cluster)
*/}}
{{- define "honr.postgres.serviceName" -}}
{{ include "honr.fullname" . }}-postgres
{{- end }}
