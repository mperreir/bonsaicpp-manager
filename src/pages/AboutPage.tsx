import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { api, type AboutInfo } from "../lib/api";
import iconUrl from "../../src-tauri/icons/128x128.png?url";

export default function AboutPage() {
  const { t } = useTranslation();
  const [info, setInfo] = useState<AboutInfo | null>(null);

  useEffect(() => {
    api.aboutInfo().then(setInfo).catch(() => {});
  }, []);

  return (
    <div className="page">
      <h2>{t("about.title")}</h2>
      <section className="card">
        <div className="row">
          <img src={iconUrl} alt="Bonsai.cpp Manager" width={64} height={64} />
          <div>
            <h3>Bonsai.cpp Manager</h3>
            <p className="muted">{t("about.by")}</p>
          </div>
        </div>
        <p>{t("about.version", { v: info?.version ?? "…" })}</p>
        <p className="muted small">{t("about.build", { date: info?.built_at ?? "…" })}</p>
      </section>
    </div>
  );
}
