// Paste this expression into CC Switch's custom usage-query editor.
// Credentials are substituted by CC Switch; do not hard-code them here.
({
  request: {
    url:
      "{{baseUrl}}".replace(/\/+$/, "").replace(/\/v1$/, "") + "/v1/usage",
    method: "GET",
    headers: {
      Authorization: "Bearer {{apiKey}}",
      "User-Agent": "cc-switch/1.0",
    },
  },
  extractor: function (response) {
    var invalid = function (message) {
      return { isValid: false, invalidMessage: String(message) };
    };
    var finite = function (value) {
      return typeof value === "number" && isFinite(value) && value >= 0;
    };
    if (!response || typeof response !== "object") {
      return invalid("额度接口未返回对象");
    }
    if (response.isValid === false || response.error) {
      return invalid(response.status || response.message || "额度查询失败");
    }
    var quota = response.quota || {};
    var unit = response.unit || quota.unit || "额度";
    if (Array.isArray(response.rate_limits) && response.rate_limits.length) {
      var windows = response.rate_limits.map(function (window) {
        if (!window || typeof window !== "object") {
          return invalid("额度窗口格式不正确");
        }
        if (
          !finite(window.limit) &&
          !finite(window.used) &&
          !finite(window.remaining)
        ) {
          return invalid("额度窗口缺少有效数值");
        }
        var names = { "5h": "5小时额度", "7d": "7天额度" };
        var result = {
          isValid: true,
          planName: names[window.window] || String(window.window || "自定义") + "额度",
          unit: unit,
          extra: "重置：" + String(window.reset_at || "服务端未提供"),
        };
        if (finite(window.limit)) result.total = window.limit;
        if (finite(window.used)) result.used = window.used;
        if (finite(window.remaining)) result.remaining = window.remaining;
        return result;
      });
      return windows;
    }
    var result = { isValid: true, unit: unit };
    if (typeof response.planName === "string") result.planName = response.planName;
    if (finite(response.remaining)) result.remaining = response.remaining;
    if (finite(quota.limit)) result.total = quota.limit;
    if (finite(quota.used)) result.used = quota.used;
    if (
      result.remaining === undefined &&
      result.total === undefined &&
      result.used === undefined
    ) {
      return invalid("额度接口格式不匹配，请按商家实际响应调整提取器");
    }
    return result;
  },
})
